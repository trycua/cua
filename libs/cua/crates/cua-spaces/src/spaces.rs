//! [`Spaces`]: the registry, the two ways to get a Space ([`Spaces::create`]
//! a new sandbox where `on` says, or [`Spaces::add`] an existing machine by
//! address), and connected [`Space`] handles. [`Spaces::delete`] undoes a
//! create; [`Spaces::remove`] forgets a Space without touching it.

use crate::error::{Error, Result};
use crate::fleet_runtime;
use crate::id::{Provider, SpaceId, authority};
use crate::operator::{ControlServerDisplay, OperatorDisplay};
use crate::registry::{Credential, Registry, cua_home};
use crate::space::{Gateway, Space, SpaceInfo, SpacePower};
use crate::space::{ServiceSource, SpaceService};
use cua_fleet::{ClaimOptions, FleetClient, PoolSpec};
use cua_proto::daemon::v1 as dpb;
use cua_sandbox_core::placement::{Kind, On, Runtime};
use cua_sandbox_core::{CreateOptions, LocalRuntime, PortTarget, Probe, ProviderKind, Sandboxes};
use cua_spaces_contract::inputs::FleetRuntime;
use cua_spacesd_client::{ConnectOptions, SpacesdClient, pb};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Default time allowed for the `GetCapabilities` handshake.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Options for [`Spaces::create`]: the same location / kind / runtime model
/// as sandboxes ([`cua_sandbox_core::placement`]).
#[derive(Clone, Debug, Default)]
pub struct SpaceCreate {
    /// Image. Default: the canonical Linux image
    /// (`ghcr.io/trycua/linux:24.04`, or `CUA_IMAGE_LINUX`).
    pub image: Option<String>,
    /// Where: `local`, `cloud` or a registered provider. `None`: the user
    /// default (`default.on`, `CUA_DEFAULT_ON`, else local).
    pub on: Option<On>,
    /// What kind of machine (`Auto`: from the image).
    pub kind: Kind,
    /// Which engine (`Auto`: the safest one for the kind).
    pub runtime: Runtime,
    /// Name. Default `space-<hex>`.
    pub name: Option<String>,
    /// vCPUs. Local default 2; Cua Cloud: 1-64
    /// ([`cua_fleet::FLEET_ABSOLUTE_CPUS`]; most accounts run 1-8), default
    /// the pool template's (4).
    pub cpus: Option<u32>,
    /// Memory in MiB. Local default 4096; Cua Cloud: 512 up to 512 GiB
    /// ([`cua_fleet::FLEET_ABSOLUTE_MEMORY_MB`]; most accounts run 1-32
    /// GiB), default the pool template's (4096).
    pub memory_mb: Option<u64>,
    /// Grow the VM's disk to this many GiB (local VMs; `None` keeps the
    /// image's size). Containers and cloud Spaces refuse it.
    pub disk_gb: Option<u32>,
    /// Readiness budget (local; default 600 s).
    pub timeout: Option<Duration>,
    /// Wait until the Space is ready. Default true; with false the result
    /// is [`SpaceCreated::Starting`].
    pub wait: Option<bool>,
    /// Return a reachable registered Space in the same location with what
    /// was asked for (services, cua-spacesd) instead of creating one.
    pub reuse: bool,
    /// Entrypoint override (container images).
    pub command: Option<Vec<String>>,
    /// Guest environment (not secrets).
    pub env: BTreeMap<String, String>,
    /// Named services (name → guest port), for example `{"mcp": 8765}`.
    pub services: BTreeMap<String, u16>,
    /// Whether the image runs cua-spacesd (see [`expects_spacesd`]).
    pub spacesd: Option<bool>,
    /// The spacesd token baked into a cloud image, if any.
    pub env_token: Option<String>,
    /// Receives what the create is doing (pulling, booting, waiting for
    /// cua-spacesd, connecting), in order, ending with `ready`. With
    /// `wait = false` reports continue after `create` returns, until the
    /// background create finishes. Every report carries the id the Space
    /// will have ([`CreateProgress::target`]).
    pub progress: Option<ProgressSink>,
    /// The caller's own key for this create (an app's pending row id):
    /// [`Spaces::cancel_create`] finds the create by it, before its name
    /// or id is known.
    pub create_id: Option<String>,
    /// A GPU option of the runtime the Space runs on (see
    /// [`Spaces::gpu_support`]): `paravirtual` for a macOS VM on Lume
    /// ("GPU acceleration"), `virgl` for QEMU, `nvidia` for a container;
    /// `auto` picks the runtime's own. `None`: no GPU. Kept with the Space
    /// (every later start applies it).
    pub gpu: Option<String>,
}

/// Where [`SpaceCreate::progress`] reports go.
#[derive(Clone)]
pub struct ProgressSink(pub cua_sandbox_core::progress::Sink);

impl ProgressSink {
    /// A sink calling `f` for every report.
    pub fn new(f: impl Fn(&CreateProgress) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
}

impl std::fmt::Debug for ProgressSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProgressSink")
    }
}

pub use cua_sandbox_core::progress::{Phase as CreatePhase, Progress as CreateProgress};

/// What [`Spaces::create`] returns. A one-off return value (never stored in
/// bulk), so the ready variant's size does not matter.
#[derive(Clone, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum SpaceCreated {
    /// The Space is ready and registered.
    Ready {
        /// The Space.
        info: SpaceInfo,
        /// `reuse` returned an existing Space.
        reused: bool,
    },
    /// `wait = false`: the Space is starting.
    Starting(PendingSpace),
}

impl SpaceCreated {
    /// The ready Space, or the pending one (`wait = false`).
    pub fn ready(self) -> std::result::Result<SpaceInfo, PendingSpace> {
        match self {
            SpaceCreated::Ready { info, .. } => Ok(info),
            SpaceCreated::Starting(p) => Err(p),
        }
    }

    /// Whether `reuse` returned an existing Space.
    pub fn reused(&self) -> bool {
        matches!(self, SpaceCreated::Ready { reused: true, .. })
    }
}

/// Options for a cloud Space (see [`Spaces::create`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct FleetClaim {
    /// Image. Default: [`fleet_runtime::default_image`].
    pub image: Option<String>,
    /// Runtime. Default: from the image's registry manifest (a container
    /// rootfs runs on gVisor, a containerDisk on KubeVirt); an explicit one
    /// must match it.
    pub runtime: Option<FleetRuntime>,
    /// Claim name. Default `space-<hex>`.
    pub name: Option<String>,
    /// Wait for the claim to bind and the spacesd to answer. Default true.
    pub wait: Option<bool>,
    /// The spacesd token baked into the image, if any. When the driver
    /// runs in bootstrap mode (no token), a fresh token is installed with
    /// `SystemService.Init` and stored.
    pub env_token: Option<String>,
    /// Entrypoint override (Fleet: `processMode: Run` on gVisor and
    /// KubeVirt).
    pub command: Option<Vec<String>>,
    /// Guest environment (Fleet: the template's `env`, not secrets).
    pub env: BTreeMap<String, String>,
    /// Named services (name → guest port), for example `{"mcp": 8765}`.
    pub services: BTreeMap<String, u16>,
    /// Whether the image runs cua-spacesd. Default: yes for the Spaces
    /// images (and no image), no for any other image. Only decides how long
    /// to wait for the driver after the sandbox is ready; a Space without
    /// it is still a Space.
    pub spacesd: Option<bool>,
    /// vCPUs (`vmTemplate.cpuCores`). Default: the template's.
    pub cpus: Option<u32>,
    /// Memory in MiB (`vmTemplate.memory`). Default: the template's.
    pub memory_mb: Option<u32>,
}

/// What [`Spaces::finish_fleet_claim`] needs from [`Spaces::claim_fleet`].
struct FleetFinish {
    claim: cua_fleet::Claim,
    runtime: cua_fleet::RuntimeKind,
    claim_token: Option<String>,
    env_token: Option<String>,
    expect_env: bool,
}

/// The spacesd token a cloud Space's claim delivers, or `None` when no
/// cua-spacesd is expected (or the guest is Windows, which has no claim
/// token channel; `guest_os` from [`fleet_runtime::guest_os`]).
///
/// A Spaces image in Fleet (the published `linux:24.04`, whose daemon is
/// still the pre-rename cua-guestd, and every newer one) mints a random
/// token of its own at boot unless Fleet mounts the claim's Secret at
/// `/run/cua`; no client could learn that token, so every call failed with
/// `missing or invalid bearer token`. The claim therefore carries this token
/// in its Secret (the template opts in with `claimSecrets`): the caller's,
/// else a fresh one.
fn fleet_claim_token(
    guest_os: &str,
    expect_env: bool,
    given: Option<&str>,
) -> Result<Option<String>> {
    if !expect_env || guest_os == "windows" {
        return Ok(None);
    }
    let token = given
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(cua_fleet::claim_secrets::generate_claim_token);
    cua_fleet::claim_secrets::validate_claim_token(&token)?;
    Ok(Some(token))
}

/// Options for a local Space (see [`Spaces::create`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct LocalProvision {
    /// Image. Default: the canonical Linux image.
    pub image: Option<String>,
    /// What kind of machine.
    pub kind: Kind,
    /// Which engine.
    pub runtime: Runtime,
    /// Instance name. Default `space-<hex>`.
    pub name: Option<String>,
    /// vCPUs. Default 2.
    pub cpus: Option<u32>,
    /// Memory (MiB). Default 4096.
    pub memory_mb: Option<u64>,
    /// VM disk size (GiB). Default: the image's.
    pub disk_gb: Option<u32>,
    /// Readiness budget. Default 600 s.
    pub timeout: Option<Duration>,
    /// Entrypoint override.
    pub command: Option<Vec<String>>,
    /// Guest environment.
    pub env: BTreeMap<String, String>,
    /// Named services (name → guest port), for example `{"mcp": 8765}`.
    pub services: BTreeMap<String, u16>,
    /// Whether the image runs cua-spacesd (see [`FleetClaim::spacesd`]).
    pub spacesd: Option<bool>,
    /// A GPU option ([`SpaceCreate::gpu`]).
    pub gpu: Option<String>,
}

/// A Space [`Spaces::create`] started with `wait = false`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PendingSpace {
    /// The id the Space will have.
    pub id: String,
    /// Always `starting` (see `mcp::tools::PHASE_STARTING`).
    pub phase: &'static str,
}

/// Builder for [`Spaces`].
#[derive(Default)]
pub struct SpacesBuilder {
    home: Option<PathBuf>,
    fleet: Option<FleetClient>,
    local: Option<Arc<dyn LocalRuntime>>,
    sandboxes: Option<Sandboxes>,
    fleet_namespace: Option<String>,
    operator_display: Option<Arc<dyn OperatorDisplay>>,
    probe_timeout: Option<Duration>,
    download_dir: Option<PathBuf>,
    #[cfg(feature = "spaces-agents")]
    relay: Option<crate::relay::RelayAccount>,
    #[cfg(feature = "mcp")]
    extensions: Vec<Arc<dyn crate::extension::SpacesExtension>>,
    #[cfg(feature = "spaces-agents")]
    share_consent: Option<Arc<dyn crate::share::ShareConsent>>,
}

impl SpacesBuilder {
    /// Who confirms `share_space` (the daemon: user presence). Without one
    /// sharing is refused.
    #[cfg(feature = "spaces-agents")]
    pub fn share_consent(mut self, consent: Arc<dyn crate::share::ShareConsent>) -> Self {
        self.share_consent = Some(consent);
        self
    }

    /// Registers an extension: the tools it serves (teleport, the Cua
    /// Drive, persistent agents) become callable on this runtime. See
    /// [`crate::extension`].
    #[cfg(feature = "mcp")]
    pub fn extension(mut self, extension: Arc<dyn crate::extension::SpacesExtension>) -> Self {
        self.extensions.push(extension);
        self
    }

    /// Enables `relay:<id>` Spaces: the machines of this account on
    /// this relay ([`Spaces::relay_machines`]).
    pub fn relay(mut self, relay: crate::relay::RelayAccount) -> Self {
        self.relay = Some(relay);
        self
    }

    /// Directory for `spaces.json` and the credential store. Default: the cua
    /// home (`$CUA_HOME`, else `~/.cua`).
    pub fn home(mut self, dir: impl Into<PathBuf>) -> Self {
        self.home = Some(dir.into());
        self
    }

    /// Enables Fleet Spaces.
    pub fn fleet(mut self, fleet: FleetClient) -> Self {
        self.fleet = Some(fleet);
        self
    }

    /// Enables Local Spaces on this runtime (the daemon passes its cua-vmm
    /// adapter).
    pub fn local_runtime(mut self, runtime: Arc<dyn LocalRuntime>) -> Self {
        self.local = Some(runtime);
        self
    }

    /// Uses an existing sandbox manager (overrides `fleet` / `local_runtime`
    /// for sandbox lifecycle; `fleet` is still used for claims).
    pub fn sandboxes(mut self, sandboxes: Sandboxes) -> Self {
        self.sandboxes = Some(sandboxes);
        self
    }

    /// Base name of the Fleet pools Spaces claims from. Default:
    /// `$CUA_SPACES_NAMESPACE`, else `cua-spaces-<client id>`.
    pub fn fleet_namespace(mut self, ns: impl Into<String>) -> Self {
        self.fleet_namespace = Some(ns.into());
        self
    }

    /// What draws Spaces on the operator's desktop. Default: the Cua Spaces
    /// app's loopback control server (`~/.cua/spaces-control.json`).
    pub fn operator_display(mut self, display: Arc<dyn OperatorDisplay>) -> Self {
        self.operator_display = Some(display);
        self
    }

    /// Handshake timeout. Default 15 s.
    pub fn probe_timeout(mut self, timeout: Duration) -> Self {
        self.probe_timeout = Some(timeout);
        self
    }

    /// Where `download` lands by default. Default `~/Downloads/cua-spaces`.
    pub fn download_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.download_dir = Some(dir.into());
        self
    }

    /// Builds.
    pub fn build(self) -> Spaces {
        let home = self.home.unwrap_or_else(cua_home);
        let sandboxes = self.sandboxes.unwrap_or_else(|| {
            let mut b = Sandboxes::builder();
            if let Some(f) = &self.fleet {
                b = b.fleet(f.clone());
            }
            if let Some(l) = &self.local {
                b = b.local(l.clone());
            }
            b.build()
        });
        let namespace = self.fleet_namespace.unwrap_or_else(|| {
            default_namespace(
                self.fleet
                    .as_ref()
                    .and_then(|f| f.config().client_id.clone()),
            )
        });
        let operator_display = self.operator_display.unwrap_or_else(|| {
            Arc::new(ControlServerDisplay::new(home.join("spaces-control.json")))
        });
        let download_dir = self.download_dir.unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Downloads")
                .join("cua-spaces")
        });
        let thumbnails = crate::thumbnails::ThumbnailCache::in_home(&home);
        Spaces {
            inner: Arc::new(Inner {
                thumbnails,
                #[cfg(feature = "mcp")]
                extensions: self.extensions,
                #[cfg(feature = "spaces-agents")]
                share_consent: std::sync::RwLock::new(self.share_consent),
                registry: Registry::new(home),
                fleet: self.fleet,
                has_local: self.local.is_some(),
                sandboxes,
                namespace,
                operator_display,
                probe_timeout: self.probe_timeout.unwrap_or(DEFAULT_PROBE_TIMEOUT),
                download_dir,
                connections: tokio::sync::Mutex::new(HashMap::new()),
                #[cfg(feature = "spaces-hotspot")]
                hotspots: tokio::sync::Mutex::new(BTreeMap::new()),
                #[cfg(feature = "spaces-agents")]
                relay: std::sync::RwLock::new(self.relay),
                relay_cache: std::sync::Mutex::new(Vec::new()),
                site_login: std::sync::RwLock::new(None),
            }),
        }
    }
}

fn default_namespace(client_id: Option<String>) -> String {
    if let Some(ns) = std::env::var("CUA_SPACES_NAMESPACE")
        .ok()
        .filter(|v| !v.is_empty())
    {
        return sanitize_label(&ns);
    }
    match client_id
        .map(|c| sanitize_label(&c))
        .filter(|c| !c.is_empty())
    {
        Some(cid) => sanitize_label(&format!("cua-spaces-{cid}")),
        None => "cua-spaces".into(),
    }
}

/// The namespace of the account's cloud sandbox (Fleet claim) `name`.
async fn cloud_namespace(fleet: &FleetClient, name: &str) -> Result<String> {
    let mut found = fleet.find_claims(name).await?;
    match found.len() {
        0 => Err(Error::NotFound(format!("cloud:{name}"))),
        1 => Ok(found.remove(0).metadata.namespace),
        n => Err(Error::invalid(format!(
            "cloud:{name} names claims in {n} pools (created before names were unique); \
             delete one of them"
        ))),
    }
}

/// How long reading a cloud Space's image may take before the Space is
/// connected without one.
const CLOUD_IMAGE_TIMEOUT: Duration = Duration::from_secs(10);

/// The image a cloud Space runs, from what the SDK already has: this
/// machine's record of the sandbox (the requested reference and its
/// resolved digest), else the claim's template (the reference as the
/// template names it; the digest only when that reference is pinned). No
/// registry read; best effort and bounded: a Space connects without it.
async fn cloud_image(
    sandboxes: &Sandboxes,
    fleet: &FleetClient,
    namespace: &str,
    claim: &str,
) -> Option<(String, String)> {
    if let Some(info) = sandboxes
        .recorded_cloud_image(claim, namespace)
        .filter(|i| !i.reference.trim().is_empty())
    {
        return Some((info.reference, info.digest));
    }
    match tokio::time::timeout(CLOUD_IMAGE_TIMEOUT, fleet.claim_image(namespace, claim)).await {
        Ok(Ok(Some((reference, _runtime)))) => Some(image_with_pinned_digest(reference)),
        Ok(Ok(None)) => None,
        Ok(Err(error)) => {
            tracing::debug!(claim, %error, "the cloud Space's image was not read");
            None
        }
        Err(_) => {
            tracing::debug!(claim, "reading the cloud Space's image timed out");
            None
        }
    }
}

/// `(reference, digest)`: the digest of a `repo@sha256:…` reference, else
/// empty (a tag names no variant).
pub(crate) fn image_with_pinned_digest(reference: String) -> (String, String) {
    let digest = reference
        .find("@sha256:")
        .map(|at| reference[at + 1..].to_string())
        .filter(|d| d.len() > "sha256:".len())
        .unwrap_or_default();
    (reference, digest)
}

/// A relay machine as a Space row (capabilities are known after connecting).
pub(crate) fn relay_info(m: &crate::relay::RelayMachine) -> SpaceInfo {
    SpaceInfo {
        id: SpaceId::Relay {
            machine_id: m.id.clone(),
        }
        .to_string(),
        name: if m.name.is_empty() {
            m.id.clone()
        } else {
            m.name.clone()
        },
        provider: Provider::Relay,
        spacesd_version: m.version.clone(),
        features: vec![],
        os: String::new(),
        os_name: String::new(),
        os_pretty_name: String::new(),
        image: String::new(),
        image_digest: String::new(),
        kind: String::new(),
        arch: String::new(),
        services: vec![],
        added_at: None,
        host: m.host.clone().unwrap_or_default(),
        host_name: String::new(),
        // A Space one of your machines provides is turned off and on by
        // that machine, which stops it (freeing the host) and boots it
        // again ([`crate::host_spaces::HostSpacesServer::set_power`]). Any
        // other machine on the relay is someone's computer: never.
        power: if m.host.as_deref().is_some_and(|h| !h.is_empty()) {
            cua_sandbox_core::PowerControl::Stop.as_str().into()
        } else {
            String::new()
        },
        power_state: String::new(),
        // A Space in someone's own cloud says so in its relay metadata;
        // the list fills in how this device can delete it.
        cloud: m
            .meta
            .get(cua_sandbox_core::byoc::meta::PROVIDER)
            .cloned()
            .unwrap_or_default(),
        cloud_place: m
            .meta
            .get(cua_sandbox_core::byoc::meta::PLACE)
            .cloned()
            .unwrap_or_default(),
        cloud_delete: String::new(),
    }
}

/// The guest's OS name and full OS string for its record: what cua-spacesd
/// reported, else the image catalog's distribution for the Space's image
/// ("Omarchy"), so a Space whose driver does not report them yet still
/// shows its distribution. Empty when neither knows.
pub(crate) fn os_names(space: &Space) -> (String, String) {
    let os = space.capabilities().os.as_ref();
    let reported = (
        os.map(|o| o.name.clone()).unwrap_or_default(),
        os.map(|o| o.pretty_name.clone()).unwrap_or_default(),
    );
    if !reported.0.trim().is_empty() || !reported.1.trim().is_empty() {
        return reported;
    }
    match space
        .image()
        .and_then(|(reference, _)| cua_image::catalog::catalog_distro(reference))
    {
        Some(d) => (d.name.clone(), d.name.clone()),
        None => reported,
    }
}

/// The guest OS family from `GetCapabilities`: `linux`, `macos`,
/// `windows`, or empty when not reported.
pub(crate) fn os_family(caps: &cua_proto::env::v1::GetCapabilitiesResponse) -> String {
    use cua_proto::env::v1::OsFamily;
    match caps.os.as_ref().map(|o| o.family()) {
        Some(OsFamily::Linux) => "linux",
        Some(OsFamily::Macos) => "macos",
        Some(OsFamily::Windows) => "windows",
        _ => "",
    }
    .into()
}

/// Lowercase DNS label: `[a-z0-9-]`, trimmed of dashes, at most 63 chars.
pub fn sanitize_label(value: &str) -> String {
    let out: String = value
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = out.trim_matches('-');
    trimmed[..trimmed.len().min(63)]
        .trim_end_matches('-')
        .to_string()
}

pub(crate) fn random_suffix() -> String {
    format!("{:010x}", rand::random::<u64>() & 0xff_ffff_ffff)
}

pub(crate) struct Inner {
    pub(crate) registry: Registry,
    /// Every Space's latest thumbnail ([`crate::thumbnails`]).
    pub(crate) thumbnails: crate::thumbnails::ThumbnailCache,
    /// The registered extensions ([`crate::extension`]).
    #[cfg(feature = "mcp")]
    pub(crate) extensions: Vec<Arc<dyn crate::extension::SpacesExtension>>,
    #[cfg(feature = "spaces-agents")]
    pub(crate) share_consent: std::sync::RwLock<Option<Arc<dyn crate::share::ShareConsent>>>,
    pub(crate) fleet: Option<FleetClient>,
    has_local: bool,
    pub(crate) sandboxes: Sandboxes,
    namespace: String,
    pub(crate) operator_display: Arc<dyn OperatorDisplay>,
    probe_timeout: Duration,
    pub(crate) download_dir: PathBuf,
    connections: tokio::sync::Mutex<HashMap<String, Space>>,
    #[cfg(feature = "spaces-hotspot")]
    pub(crate) hotspots: tokio::sync::Mutex<BTreeMap<String, crate::hotspot::Hotspot>>,
    #[cfg(feature = "spaces-agents")]
    relay: std::sync::RwLock<Option<crate::relay::RelayAccount>>,
    /// The last directory listing (so `list` and `resolve` stay sync).
    relay_cache: std::sync::Mutex<Vec<crate::relay::RelayMachine>>,
    /// The Keyvault broker `request_site_login` signs in through (the
    /// daemon sets it once its Keyvault is up).
    site_login: std::sync::RwLock<Option<Arc<dyn crate::site_login::SiteLoginBroker>>>,
}

/// The Spaces runtime: a registry of spacesd sandboxes and the operations
/// on them. Cheap to clone. Hosted in-process by apps, or by `cua daemon`
/// (which also serves [`crate::mcp`] from it).
#[derive(Clone)]
pub struct Spaces {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Spaces {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spaces")
            .field("home", &self.inner.registry.dir())
            .field("fleet", &self.inner.fleet.is_some())
            .field("local", &self.inner.has_local)
            .field("relay", &self.relay_account().map(|r| r.url))
            .finish()
    }
}

impl Spaces {
    /// A builder.
    pub fn builder() -> SpacesBuilder {
        SpacesBuilder::default()
    }

    /// The cua home this runtime keeps its state in.
    pub fn home_dir(&self) -> &std::path::Path {
        self.inner.registry.dir()
    }

    /// The registry.
    pub fn registry(&self) -> &Registry {
        &self.inner.registry
    }

    /// The Fleet client, when configured.
    pub fn fleet(&self) -> Option<&FleetClient> {
        self.inner.fleet.as_ref()
    }

    /// The sandbox manager Local Spaces are provisioned with.
    pub fn sandboxes(&self) -> &Sandboxes {
        &self.inner.sandboxes
    }

    /// The operator display.
    pub fn operator_display(&self) -> &Arc<dyn OperatorDisplay> {
        &self.inner.operator_display
    }

    /// Base name of the Fleet pools Spaces claims from.
    pub fn fleet_namespace(&self) -> &str {
        &self.inner.namespace
    }

    fn fleet_client(&self) -> Result<&FleetClient> {
        self.inner.fleet.as_ref().ok_or_else(|| {
            Error::host(
                cua_spaces_contract::host::FLEET,
                "set CUA_CLIENT_ID/CUA_CLIENT_SECRET (or CUA_TOKEN) to use cloud Spaces",
            )
        })
    }

    // ------------------------------------------------------------ registry

    /// Every registered Space, plus the relay machines of the last
    /// [`Spaces::relay_machines`] listing (see [`Spaces::list_all`]).
    pub fn list(&self) -> Result<Vec<SpaceInfo>> {
        let mut out: Vec<SpaceInfo> = self
            .inner
            .registry
            .list()?
            .iter()
            .map(|r| {
                let mut info = SpaceInfo::from_record(r)?;
                (info.power, info.power_state) = self.power_of(&info.id);
                Ok(info)
            })
            .collect::<Result<_>>()?;
        let cache = self.inner.relay_cache.lock().expect("relay cache");
        for m in cache.iter() {
            let mut info = relay_info(m);
            if !info.host.is_empty() {
                info.host_name = cache
                    .iter()
                    .find(|h| h.id == info.host)
                    .map(|h| {
                        if h.name.is_empty() {
                            h.id.clone()
                        } else {
                            h.name.clone()
                        }
                    })
                    .unwrap_or_default();
            } else if let Some((provider, place)) =
                self.inner.sandboxes.cloud_of_relay_machine(&m.id)
            {
                // A Space in your cloud that this home created: deleted here.
                info.cloud = provider;
                info.cloud_place = place.clone();
                info.host_name = place;
                info.cloud_delete = crate::cloud::DELETE_HERE.into();
                // Whether its instance stops and starts (a VM does; Modal
                // cannot).
                if let Ok(Some(sandbox)) = self.inner.sandboxes.by_relay_machine(&m.id) {
                    (info.power, info.power_state) = self.sandbox_power(&sandbox);
                }
            } else if !info.cloud.is_empty() {
                // One another device of the account created (its relay
                // metadata): that device deletes it, when it can be asked.
                info.host_name = info.cloud_place.clone();
                info.cloud_delete = match m.meta.get(cua_sandbox_core::byoc::meta::HOST) {
                    Some(host) if !host.is_empty() => format!("host:{host}"),
                    _ => crate::cloud::DELETE_ELSEWHERE.into(),
                };
            }
            if !out.iter().any(|i| i.id == info.id) {
                out.push(info);
            }
        }
        Ok(out)
    }

    /// Refreshes the relay directory (when a relay account is configured;
    /// a failure is logged, not fatal) and lists every Space.
    pub async fn list_all(&self) -> Result<Vec<SpaceInfo>> {
        if self.relay_account().is_some()
            && let Err(e) = self.relay_machines().await
        {
            // Not signed in is the normal state of many hosts.
            if e.tag() == "unauthenticated" {
                tracing::debug!(error = %e, "relay directory skipped");
            } else {
                tracing::warn!(error = %e, "relay directory unavailable");
            }
        }
        self.list()
    }

    // --------------------------------------------------------------- relay

    /// The relay account, when configured.
    pub fn relay_account(&self) -> Option<crate::relay::RelayAccount> {
        self.inner.relay.read().expect("relay").clone()
    }

    /// Wires in (or clears) the Keyvault broker `request_site_login` signs
    /// in through. The daemon sets it once its Keyvault is up; without one
    /// the tool is fail-closed.
    pub fn set_site_login_broker(
        &self,
        broker: Option<Arc<dyn crate::site_login::SiteLoginBroker>>,
    ) {
        *self.inner.site_login.write().expect("site login broker") = broker;
    }

    /// The Keyvault broker `request_site_login` signs in through, when one
    /// is wired in (the daemon's).
    pub fn site_login_broker(&self) -> Option<Arc<dyn crate::site_login::SiteLoginBroker>> {
        self.inner
            .site_login
            .read()
            .expect("site login broker")
            .clone()
    }

    /// Sets who confirms `share_space` after construction (the `cua` CLI
    /// gives an embedded runtime a terminal prompt).
    #[cfg(feature = "spaces-agents")]
    pub fn set_share_consent(&self, consent: Option<Arc<dyn crate::share::ShareConsent>>) {
        *self.inner.share_consent.write().expect("share consent") = consent;
    }

    /// Whether something here can confirm a share.
    #[cfg(feature = "spaces-agents")]
    pub fn has_share_consent(&self) -> bool {
        self.inner
            .share_consent
            .read()
            .expect("share consent")
            .is_some()
    }

    /// Configures (or clears) the relay account after construction, e.g.
    /// when `cua daemon` picks up the signed-in session. Clears the cached
    /// directory listing.
    pub fn set_relay(&self, relay: Option<crate::relay::RelayAccount>) {
        *self.inner.relay.write().expect("relay") = relay;
        self.inner.relay_cache.lock().expect("relay cache").clear();
    }

    pub(crate) fn relay(&self) -> Result<crate::relay::RelayAccount> {
        self.relay_account().ok_or_else(|| {
            Error::host(
                "cua.ai relay account",
                "sign in to cua.ai (`cua auth login` or the app) to list your machines",
            )
        })
    }

    /// The machines of the signed-in account on the relay (owned and shared
    /// with it), each reachable as `relay:<id>`.
    pub async fn relay_machines(&self) -> Result<Vec<crate::relay::RelayMachine>> {
        let relay = self.relay()?;
        let token = relay.tokens.access_token().await?;
        let machines = relay.client().await?.machines(&token).await?;
        *self.inner.relay_cache.lock().expect("relay cache") = machines.clone();
        Ok(machines)
    }

    /// Resolves any accepted spelling (canonical id, legacy id, URL, display
    /// name, short name) to a canonical id.
    pub fn resolve(&self, space: &str) -> Result<SpaceId> {
        if let Ok(id) = SpaceId::parse(space) {
            return Ok(id);
        }
        let records = self.inner.registry.list()?;
        let mut matches: Vec<SpaceId> = records
            .iter()
            .filter_map(|r| SpaceId::parse(&r.id).ok().map(|id| (r, id)))
            .filter(|(r, id)| r.name == space || id.short_name() == space)
            .map(|(_, id)| id)
            .collect();
        for m in self.inner.relay_cache.lock().expect("relay cache").iter() {
            let id = SpaceId::Relay {
                machine_id: m.id.clone(),
            };
            if (m.name == space || m.id == space) && !matches.contains(&id) {
                matches.push(id);
            }
        }
        match matches.as_slice() {
            [one] => Ok(one.clone()),
            [] => Err(Error::NotFound(format!(
                "Space {space:?} (use its id, such as local:<name> or cloud:<name>, or add it \
                 with add_space)"
            ))),
            _ => {
                let mut refs: Vec<cua_sandbox_core::SandboxRef> =
                    matches.iter().map(SpaceId::to_ref).collect();
                refs.sort_by_key(|r| (r.location(), r.to_string()));
                Err(Error::Sandbox(cua_sandbox_core::Error::AmbiguousSandbox {
                    message: cua_sandbox_core::refs::ambiguous_message(space, &refs),
                    name: space.to_string(),
                    candidates: refs.iter().map(ToString::to_string).collect(),
                }))
            }
        }
    }

    /// Adds a Space by URL and stores it.
    ///
    /// `url` is `http(s)://host:port` (direct), bare `host:port`, or a
    /// Space id (`cloud:<name>`, `local:<name>`, `direct:<host:port>`, or a
    /// legacy `space://…` spelling). A URL that answers the
    /// spacesd `GetCapabilities` handshake is a spacesd Space; any
    /// other URL that answers MCP `initialize` (for example
    /// `http://host:8765/mcp`) becomes a Space with one service, `mcp`, and
    /// an empty capability set. Re-adding updates the entry.
    pub async fn add(
        &self,
        url: &str,
        token: Option<String>,
        name: Option<String>,
    ) -> Result<SpaceInfo> {
        self.add_with_service(url, token, name, None).await
    }

    /// [`Spaces::add`], naming the service a plain MCP URL is registered
    /// under (default `mcp`).
    pub async fn add_with_service(
        &self,
        url: &str,
        token: Option<String>,
        name: Option<String>,
        service: Option<String>,
    ) -> Result<SpaceInfo> {
        let url = url.trim();
        let is_id = url.starts_with("space://")
            || ["local:", "cloud:", "fleet:", "direct:", "relay:"]
                .iter()
                .any(|p| url.starts_with(p));
        let (id, endpoint_url) = if is_id {
            let id = SpaceId::parse(url)?;
            let endpoint = match &id {
                SpaceId::Direct { authority } => Some(format!("http://{authority}")),
                _ => None,
            };
            (id, endpoint)
        } else {
            let with_scheme = if url.contains("://") {
                url.to_string()
            } else {
                format!("http://{url}")
            };
            let endpoint = cua_spacesd_client::Endpoint::parse(&with_scheme)?;
            let id = SpaceId::Direct {
                authority: authority(endpoint.host(), endpoint.port()),
            };
            (id, Some(with_scheme))
        };
        let mut credential = Credential {
            url: endpoint_url.clone(),
            token,
            ..Default::default()
        };
        let space = match self.connect(&id, &credential, name.as_deref()).await {
            Ok(s) => s,
            Err(e) if not_an_spacesd(&e) => {
                // Not a spacesd: maybe an MCP endpoint.
                let (Some(mcp_url), SpaceId::Direct { .. }) = (endpoint_url, &id) else {
                    return Err(e);
                };
                let service = service
                    .filter(|s| !s.trim().is_empty())
                    .unwrap_or_else(|| "mcp".into());
                credential.service_urls.insert(service.clone(), mcp_url);
                let space = self.connect(&id, &credential, name.as_deref()).await?;
                if let Err(mcp) = space.mcp(&service).await {
                    return Err(match e {
                        Error::SpacesdNotAvailable { reason, .. } => Error::SpacesdNotAvailable {
                            space: id.to_string(),
                            reason: format!("{reason}; no MCP server answered either ({mcp})"),
                        },
                        other => other,
                    });
                }
                space
            }
            Err(e) => return Err(e),
        };
        self.store(&space, credential)?;
        self.inner
            .connections
            .lock()
            .await
            .insert(id.to_string(), space.clone());
        self.info(&id)
    }

    /// Forgets a Space (registry entry and stored token). The sandbox is not
    /// touched.
    pub async fn remove(&self, space: &str) -> Result<SpaceInfo> {
        let id = self.resolve(space)?;
        let info = self.info(&id).ok();
        self.drop_connection(&id).await;
        // A direct host, or a Space one created, leaves the direct-host list.
        if matches!(id, SpaceId::Direct { .. }) {
            let _ = self.inner.registry.forget_direct(&id.to_string());
        }
        if !self.inner.registry.remove(&id.to_string())? && !matches!(id, SpaceId::Relay { .. }) {
            return Err(Error::NotFound(id.to_string()));
        }
        self.inner.thumbnails.remove(&id.to_string());
        Ok(info.unwrap_or_else(|| SpaceInfo {
            id: id.to_string(),
            name: id.short_name().into(),
            provider: id.provider(),
            spacesd_version: String::new(),
            features: vec![],
            os: String::new(),
            os_name: String::new(),
            os_pretty_name: String::new(),
            image: String::new(),
            image_digest: String::new(),
            kind: String::new(),
            arch: String::new(),
            services: vec![],
            added_at: None,
            host: String::new(),
            host_name: String::new(),
            power: String::new(),
            power_state: String::new(),
            cloud: String::new(),
            cloud_place: String::new(),
            cloud_delete: String::new(),
        }))
    }

    pub(crate) async fn drop_connection(&self, id: &SpaceId) {
        #[cfg(feature = "spaces-hotspot")]
        if let Some(h) = self.inner.hotspots.lock().await.remove(&id.to_string()) {
            let _ = h.stop().await;
        }
        // What an extension holds for the Space (a mounted volume) ends
        // while the Space still answers.
        #[cfg(feature = "mcp")]
        for e in &self.inner.extensions {
            e.space_dropped(self, &id.to_string()).await;
        }
        self.inner.connections.lock().await.remove(&id.to_string());
    }

    pub(crate) fn info(&self, id: &SpaceId) -> Result<SpaceInfo> {
        if let SpaceId::Relay { machine_id } = id
            && let Some(m) = self
                .inner
                .relay_cache
                .lock()
                .expect("relay cache")
                .iter()
                .find(|m| &m.id == machine_id)
            && self.inner.registry.get(&id.to_string())?.is_none()
        {
            return Ok(relay_info(m));
        }
        let record = self
            .inner
            .registry
            .get(&id.to_string())?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        SpaceInfo::from_record(&record)
    }

    /// Brings a registered Space's handshake facts (OS, image) up to date
    /// after a connect, when they changed (a newer driver, a record written
    /// before these fields existed). Best effort: the connect stands.
    fn refresh_record(&self, key: &str, space: &Space, credential: &Credential) {
        let Ok(Some(record)) = self.inner.registry.get(key) else {
            return;
        };
        let (image, digest) = space.image().unwrap_or_default();
        let (os_name, os_pretty_name) = os_names(space);
        let (kind, arch) = space.platform();
        let stale = record.os_pretty_name != os_pretty_name
            || record.os_name != os_name
            || record.kind != kind
            || record.arch != arch
            || (!image.is_empty() && (record.image != image || record.image_digest != digest));
        if stale
            && space.has_spacesd()
            && let Err(error) = self.store(space, credential.clone())
        {
            tracing::debug!(space = key, %error, "could not refresh the Space's record");
        }
    }

    fn store(&self, space: &Space, credential: Credential) -> Result<()> {
        let caps = space.capabilities();
        let existing = self.inner.registry.get(&space.id().to_string())?;
        let (os_name, os_pretty_name) = os_names(space);
        let (kind, arch) = space.platform();
        let record = dpb::Space {
            kind,
            arch,
            id: space.id().to_string(),
            name: space.name().to_string(),
            spacesd_version: caps.version.clone(),
            features: caps
                .features
                .iter()
                .filter(|f| f.supported)
                .map(|f| f.name.clone())
                .collect(),
            os: os_family(caps),
            os_name,
            os_pretty_name,
            image: space
                .image()
                .map(|(r, _)| r.to_string())
                .unwrap_or_default(),
            image_digest: space
                .image()
                .map(|(_, d)| d.to_string())
                .unwrap_or_default(),
            services: space.declared_services().keys().cloned().collect(),
            host: existing
                .as_ref()
                .map(|r| r.host.clone())
                .unwrap_or_default(),
            host_name: existing
                .as_ref()
                .map(|r| r.host_name.clone())
                .unwrap_or_default(),
            added_at: existing.and_then(|e| e.added_at).or_else(|| {
                let d = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default();
                Some(pbjson_types::Timestamp {
                    seconds: d.as_secs() as i64,
                    nanos: d.subsec_nanos() as i32,
                })
            }),
            // Power is filled in when listed ([`Spaces::power_of`]), never
            // stored; the cloud fields when listed too.
            ..Default::default()
        };
        self.inner.registry.upsert(record, credential)
    }

    // --------------------------------------------------------- connections

    /// A connected handle to a registered Space (cached; reconnects when the
    /// cached connection's health check fails).
    pub async fn space(&self, space: &str) -> Result<Space> {
        let id = self.resolve(space)?;
        let key = id.to_string();
        if let Some(s) = self.inner.connections.lock().await.get(&key).cloned() {
            return Ok(s);
        }
        let name = match (&id, self.inner.registry.get(&key)?) {
            (_, Some(record)) => Some(record.name),
            (SpaceId::Relay { machine_id }, None) => Some(
                self.inner
                    .relay_cache
                    .lock()
                    .expect("relay cache")
                    .iter()
                    .find(|m| &m.id == machine_id)
                    .map(|m| m.name.clone())
                    .unwrap_or_default(),
            ),
            (_, None) => {
                return Err(Error::NotFound(format!(
                    "Space {key} is not registered (add it with add_space)"
                )));
            }
        };
        let credential = self.inner.registry.credential(&key)?.unwrap_or_default();
        let s = self.connect(&id, &credential, name.as_deref()).await?;
        self.refresh_record(&key, &s, &credential);
        self.inner.connections.lock().await.insert(key, s.clone());
        #[cfg(feature = "mcp")]
        self.notify_connected(&id);
        Ok(s)
    }

    /// Drops the cached connection so the next [`Spaces::space`] reconnects
    /// (and re-reads capabilities).
    pub async fn forget_connection(&self, space: &str) -> Result<()> {
        let id = self.resolve(space)?;
        // What an extension attached over its own socket (a mounted volume,
        // where an agent's home may be) stays; the next connection tells
        // the extensions again.
        self.inner.connections.lock().await.remove(&id.to_string());
        Ok(())
    }

    async fn connect(
        &self,
        id: &SpaceId,
        credential: &Credential,
        name: Option<&str>,
    ) -> Result<Space> {
        self.connect_with(id, credential, name, self.inner.probe_timeout)
            .await
    }

    /// Connects to a Space: its declared services, plus cua-spacesd when
    /// the image runs it. `probe` bounds the spacesd handshake. Without
    /// a spacesd, a Local or Fleet Space (and a direct Space registered
    /// with service URLs) is still a Space, with an empty capability set;
    /// a direct URL with neither fails with [`Error::SpacesdNotAvailable`].
    async fn connect_with(
        &self,
        id: &SpaceId,
        credential: &Credential,
        name: Option<&str>,
        probe: Duration,
    ) -> Result<Space> {
        let mut services: BTreeMap<String, SpaceService> = BTreeMap::new();
        let mut image: Option<(String, String)> = None;
        let mut platform: Option<(String, String)> = None;
        // The token the spacesd connection uses: the stored one, or for a
        // Local Space without one, the sandbox's own.
        let mut effective_token = credential.token.clone();
        let bearer: Vec<(String, String)> = credential
            .token
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(|t| vec![("authorization".to_string(), format!("Bearer {t}"))])
            .unwrap_or_default();
        for (svc, url) in &credential.service_urls {
            let config = cua_sandbox_core::McpConfig::for_url(url, bearer.clone())?;
            let (base, path) = split_mcp_url(&config.url)?;
            services.insert(
                svc.clone(),
                SpaceService {
                    source: ServiceSource::Url(cua_sandbox_core::ServiceEndpoint {
                        url: base,
                        headers: bearer.clone(),
                    }),
                    mcp_path: path,
                },
            );
        }
        // How to reach the spacesd, if the target can have one.
        let (options, gateway) = match id {
            SpaceId::Direct { authority } => {
                if credential.service_urls.is_empty() {
                    let url = credential
                        .url
                        .clone()
                        .unwrap_or_else(|| format!("http://{authority}"));
                    let mut o = ConnectOptions::parse(&url)?;
                    o.token = credential.token.clone();
                    (Some(o), None)
                } else {
                    // Added by an MCP URL: the token belongs to that server.
                    (None, None)
                }
            }
            SpaceId::Cloud { name, namespace } => {
                let fleet = self.fleet_client()?;
                let namespace = match namespace.clone().or_else(|| credential.namespace.clone()) {
                    Some(ns) => ns,
                    None => cloud_namespace(fleet, name).await?,
                };
                let claim = name;
                let bound = fleet.attach_claim(&namespace, claim).await?;
                image = cloud_image(&self.inner.sandboxes, fleet, &namespace, claim).await;
                for svc in bound.services.iter().filter(|s| s.as_str() != "env") {
                    services.insert(
                        svc.clone(),
                        SpaceService {
                            source: ServiceSource::Fleet {
                                fleet: fleet.clone(),
                                bound: bound.clone(),
                                service: svc.clone(),
                            },
                            mcp_path: cua_sandbox_core::mcp::DEFAULT_PATH.into(),
                        },
                    );
                }
                let gateway = Some(Gateway::Fleet {
                    fleet: fleet.clone(),
                    claim: claim.clone(),
                });
                if bound.services.iter().any(|s| s == "env") {
                    let o = fleet.env_connect_options(&bound, "env", credential.token.clone())?;
                    (Some(o), gateway)
                } else {
                    (None, gateway)
                }
            }
            SpaceId::Relay { machine_id } => {
                let relay = self.relay()?;
                let url = cua_host::RelayClient::new(&relay.url)?.machine_url(machine_id);
                // The account token rides in `authorization` and this
                // device's session next to it, fetched (and refreshed) per
                // call; the relay checks both and swaps them for its signed
                // identity assertion.
                let bearer = crate::relay::RelayBearer { account: relay };
                let mut o = ConnectOptions::parse(&url)?.fleet_gateway(Arc::new(bearer), None);
                o.token = credential.token.clone();
                (Some(o), None)
            }
            SpaceId::Local { name } => {
                let sandbox = self.inner.sandboxes.connect(name).await?;
                image = sandbox
                    .image_info()
                    .map(|i| (i.reference.clone(), i.digest.clone()));
                platform = sandbox.image_info().map(|i| {
                    let kind = match i.variant.as_str() {
                        "rootfs" => "container",
                        "containerdisk" | "lume" => "vm",
                        _ => "",
                    };
                    (kind.to_string(), i.arch.clone().unwrap_or_default())
                });
                for (svc, _) in sandbox
                    .services()
                    .iter()
                    .filter(|(s, _)| s.as_str() != "env")
                {
                    if let Ok(service) = sandbox.service(svc) {
                        services.insert(
                            svc.clone(),
                            SpaceService {
                                source: ServiceSource::Sandbox(service),
                                mcp_path: cua_sandbox_core::mcp::DEFAULT_PATH.into(),
                            },
                        );
                    }
                }
                let env_port = sandbox
                    .services()
                    .get("env")
                    .copied()
                    .unwrap_or(cua_proto::SPACESD_DEFAULT_PORT);
                // A sandbox created through the sandboxes API (`cua sb
                // create`, MCP `sandbox_create`) holds its own spacesd token:
                // add_space local:<name> needs none of its own.
                if effective_token.is_none() {
                    if sandbox.env_token().is_none() {
                        // A bootstrap-mode spacesd: installs and records one.
                        let _ = sandbox.spacesd().await;
                    }
                    effective_token = sandbox.env_token();
                }
                let token = effective_token.clone();
                match sandbox.port(env_port) {
                    Ok(PortTarget::Addr { host, port }) => {
                        let mut o =
                            ConnectOptions::parse(&format!("http://{}", authority(&host, port)))?;
                        o.token = token;
                        (Some(o), None)
                    }
                    Ok(PortTarget::Url(u)) => {
                        let mut o = ConnectOptions::parse(&u)?;
                        o.token = token;
                        (Some(o), None)
                    }
                    // The image publishes no spacesd port.
                    Err(_) => (None, None),
                }
            }
        };
        let env = match options {
            None => None,
            Some(options) => match SpacesdClient::connect(options.probe_timeout(probe)).await {
                Ok(env) => Some(env),
                Err(cua_spacesd_client::Error::SpacesdNotAvailable { reason, .. }) => {
                    let generic_ok = matches!(id, SpaceId::Local { .. } | SpaceId::Cloud { .. })
                        || !services.is_empty();
                    if !generic_ok {
                        return Err(Error::SpacesdNotAvailable {
                            space: id.to_string(),
                            reason,
                        });
                    }
                    tracing::debug!(space = %id, %reason, "no cua-spacesd; a Space of declared services");
                    None
                }
                Err(e) => return Err(e.into()),
            },
        };
        let caps = match &env {
            Some(env) => env.capabilities().await?,
            None => Default::default(),
        };
        let name = name
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                if caps.hostname.is_empty() {
                    id.short_name().to_string()
                } else {
                    caps.hostname.clone()
                }
            });
        let token = if env.is_some() { effective_token } else { None };
        let space = Space::new(id.clone(), name, env, caps, token, gateway, services);
        if let Some((reference, digest)) = image {
            space.set_image(reference, digest);
        }
        if let Some((kind, arch)) = platform {
            space.set_platform(kind, arch);
        }
        Ok(space)
    }

    /// Connects once the sandbox is ready. When the image is expected to run
    /// cua-spacesd, retries until the driver answers or `budget` runs out
    /// (the driver may start a moment after the port is published; bounded:
    /// at most `budget / 2s` attempts). Otherwise one bounded handshake, and
    /// a Space of declared services when no driver answers.
    async fn connect_when_ready(
        &self,
        id: &SpaceId,
        credential: &Credential,
        budget: Duration,
        expect_spacesd: bool,
    ) -> Result<Space> {
        if !expect_spacesd {
            let probe = self
                .inner
                .probe_timeout
                .min(budget.max(Duration::from_secs(1)));
            return self.connect_with(id, credential, None, probe).await;
        }
        let deadline = tokio::time::Instant::now() + budget;
        let max_attempts = (budget.as_secs() / 2).max(1) + 1;
        let mut last = None;
        for _ in 0..max_attempts {
            match self.connect(id, credential, None).await {
                Ok(s) if s.has_spacesd() => return Ok(s),
                Ok(_) => last = Some("cua-spacesd did not answer yet".to_string()),
                Err(e @ Error::Env(cua_spacesd_client::Error::Unauthenticated(_))) => {
                    return Err(e);
                }
                Err(e) => last = Some(e.to_string()),
            }
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        Err(Error::Timeout(format!(
            "{id}: spacesd did not answer within {budget:?} ({}); pass spacesd=false \
             for an image without it",
            last.unwrap_or_default()
        )))
    }

    // ---------------------------------------------------------- provisioning

    /// Claims a Fleet Space: validates the runtime/image pairing (the one
    /// place it is validated), reconciles a warm pool for the image and its
    /// command/env/services, claims from it, waits for the claim to bind
    /// (Fleet readiness: the spacesd port for the Spaces images, else the
    /// first declared service, else none), connects, and registers the
    /// Space. An image without cua-spacesd becomes a Space of its
    /// declared services within the handshake timeout.
    pub(crate) async fn claim_fleet(
        &self,
        opts: FleetClaim,
    ) -> Result<std::result::Result<SpaceInfo, PendingSpace>> {
        let fleet = self.fleet_client()?.clone();
        let expect_env = expects_spacesd(opts.image.as_deref(), opts.spacesd);
        let (runtime, image, kind, resolved_os) = match (opts.runtime, opts.image.clone()) {
            (Some(r), image) => {
                let image = image.unwrap_or_else(|| fleet_runtime::default_image(r));
                let got = fleet_runtime::resolve_image(Some(r), &image).await?;
                let os = got.resolved.map(|i| i.os);
                (r, image, got.runtime, os)
            }
            // No runtime: the image's manifest decides (a container rootfs
            // runs on gVisor, a containerDisk on KubeVirt), so the default
            // image (rootfs + disk) is a gVisor container.
            (None, image) => {
                let image =
                    image.unwrap_or_else(|| fleet_runtime::default_image(FleetRuntime::Gvisor));
                let got = fleet_runtime::resolve_image(None, &image).await?;
                let os = got.resolved.map(|i| i.os);
                let kind = got.runtime;
                let r = match kind {
                    cua_fleet::RuntimeKind::Kubevirt => FleetRuntime::Kubevirt,
                    cua_fleet::RuntimeKind::Gvisor => FleetRuntime::Gvisor,
                    other => {
                        return Err(Error::invalid(format!(
                            "{image} needs the Fleet runtime {other:?}, which Spaces do not claim"
                        )));
                    }
                };
                (r, image, kind, os)
            }
        };
        let guest_os = fleet_runtime::guest_os(&image, resolved_os.as_deref());
        // command and env run on both runtimes: the template carries them
        // with `processMode: Run` (see `cua_fleet::SandboxSpec`).
        let command = opts.command.clone().filter(|c| !c.is_empty());
        let mut services = opts.services.clone();
        services.remove("env");
        // The spacesd token, delivered with the claim (see
        // `fleet_claim_token`). The pool keeps its name: `apply` turns on
        // the template's claimSecrets, and Fleet binds only replicas of the
        // current template, so none started without the Secret mount.
        let claim_token = fleet_claim_token(&guest_os, expect_env, opts.env_token.as_deref())?;
        let key = sized_pool_key(
            &image,
            command.as_deref(),
            &opts.env,
            &services,
            opts.cpus,
            opts.memory_mb,
        );
        let pool_name = self.pool_name(runtime, &key);
        let mut spec = PoolSpec::new(pool_name, &image).runtime(kind.clone());
        spec.cpu = opts.cpus;
        spec.memory_mb = opts.memory_mb;
        spec.command = command;
        spec.env = opts.env.clone();
        spec.claim_secrets = claim_token.is_some();
        spec.readiness_tcp_port = if expect_env {
            Some(cua_proto::SPACESD_DEFAULT_PORT)
        } else {
            services.values().next().copied()
        };
        if expect_env {
            services.insert("env".into(), cua_proto::SPACESD_DEFAULT_PORT);
        }
        spec.services = services;
        let (sandbox, pool_options) = spec.parts();
        let pool = fleet.apply(&spec.name, &sandbox, &pool_options).await?;
        let claim_name = sanitize_label(
            &opts
                .name
                .clone()
                .unwrap_or_else(|| format!("space-{}", random_suffix())),
        );
        cua_sandbox_core::progress::report(cua_sandbox_core::progress::Progress::phase(
            cua_sandbox_core::progress::Phase::Creating,
        ));
        let (claim, created) = fleet
            .claim(
                &pool.pool,
                ClaimOptions {
                    name: Some(claim_name.clone()),
                    claim_token: claim_token.clone(),
                    ..Default::default()
                },
            )
            .await?;
        if created {
            crate::creating::record(crate::creating::Made::FleetClaim {
                namespace: claim.metadata.namespace.clone(),
                name: claim.metadata.name.clone(),
            });
        }
        let finish = FleetFinish {
            claim,
            runtime: kind,
            // An existing claim of that name keeps the token it was
            // created with; ours never reached it.
            claim_token: claim_token.filter(|_| created),
            env_token: opts.env_token.clone(),
            expect_env,
        };
        if !opts.wait.unwrap_or(true) {
            // Finish (bind, token delivery, connect, register) in the
            // background, like a local Space: the token lives only in this
            // process until the Space is registered with it.
            let id = SpaceId::Cloud {
                name: finish.claim.metadata.name.clone(),
                namespace: Some(finish.claim.metadata.namespace.clone()),
            };
            let spaces = self.clone();
            let sink = cua_sandbox_core::progress::current();
            tokio::spawn(cua_sandbox_core::progress::carry(sink, async move {
                match spaces.finish_fleet_claim(fleet, finish).await {
                    Ok(_) => cua_sandbox_core::progress::report(
                        cua_sandbox_core::progress::Progress::phase(
                            cua_sandbox_core::progress::Phase::Ready,
                        ),
                    ),
                    Err(e) => tracing::warn!(error = %e, "background cloud Space create failed"),
                }
            }));
            return Ok(Err(PendingSpace {
                id: id.to_string(),
                phase: "starting",
            }));
        }
        self.finish_fleet_claim(fleet, finish).await.map(Ok)
    }

    /// The rest of [`Self::claim_fleet`] once the claim exists: waits for it
    /// to bind and for its token to reach the driver, connects and
    /// registers the Space. The claim is released on any failure.
    async fn finish_fleet_claim(&self, fleet: FleetClient, f: FleetFinish) -> Result<SpaceInfo> {
        let FleetFinish {
            claim,
            runtime,
            claim_token,
            env_token,
            expect_env,
        } = f;
        cua_sandbox_core::progress::report(cua_sandbox_core::progress::Progress::phase(
            cua_sandbox_core::progress::Phase::Booting,
        ));
        let bound = match fleet.wait_claim(&claim).await {
            Ok(b) => b,
            Err(e) => {
                let _ = fleet
                    .release(&claim.metadata.namespace, &claim.metadata.name)
                    .await;
                return Err(e.into());
            }
        };
        if let Some(token) = &claim_token
            && let Err(e) = fleet.await_claim_secrets(&bound, token, &runtime).await
        {
            let _ = fleet.release(&bound.namespace, &bound.claim).await;
            return Err(e.into());
        }
        let id = SpaceId::Cloud {
            name: bound.claim.clone(),
            namespace: Some(bound.namespace.clone()),
        };
        let mut credential = Credential {
            url: None,
            token: claim_token.or(env_token),
            ..Default::default()
        };
        // The pool's readiness probe already waited for the driver's port;
        // this only covers the driver finishing its start.
        cua_sandbox_core::progress::report(cua_sandbox_core::progress::Progress::phase(
            cua_sandbox_core::progress::Phase::Connecting,
        ));
        let space = match self
            .connect_when_ready(&id, &credential, Duration::from_secs(120), expect_env)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let _ = fleet.release(&bound.namespace, &bound.claim).await;
                return Err(e);
            }
        };
        let space = if space.has_spacesd()
            && !space.capabilities().initialized
            && credential.token.is_none()
        {
            // Bootstrap mode: install our own token so the Space is never
            // left open, then reconnect with it.
            let token = format!("{:032x}", rand::random::<u128>());
            space
                .spacesd()?
                .init(pb::InitRequest {
                    token: token.clone(),
                    ..Default::default()
                })
                .await?;
            credential.token = Some(token);
            self.connect(&id, &credential, None).await?
        } else {
            space
        };
        self.store(&space, credential)?;
        self.inner
            .connections
            .lock()
            .await
            .insert(id.to_string(), space);
        #[cfg(feature = "mcp")]
        self.notify_connected(&id);
        self.info(&id)
    }

    /// A registered Space in `provider` that is reachable and has what was
    /// asked for (the declared `services`; a healthy cua-spacesd when one is
    /// expected).
    async fn reusable(
        &self,
        provider: Provider,
        services: &BTreeMap<String, u16>,
        expect_env: bool,
    ) -> Result<Option<SpaceInfo>> {
        let opts = FleetClaim {
            services: services.clone(),
            ..Default::default()
        };
        for info in self.list()? {
            if info.provider != provider {
                continue;
            }
            let Ok(space) = self.space(&info.id).await else {
                continue;
            };
            // Reuse only a Space that has what was asked for.
            if !opts
                .services
                .keys()
                .filter(|s| s.as_str() != "env")
                .all(|s| space.declared_services().contains_key(s))
            {
                continue;
            }
            let usable = if expect_env {
                match space.spacesd() {
                    Ok(env) => env.health().await.is_ok(),
                    Err(_) => false,
                }
            } else {
                !opts.services.is_empty()
            };
            if usable {
                return Ok(Some(info));
            }
        }
        Ok(None)
    }

    /// Creates a Space where `opts.on` says (the user default when unset):
    /// a new sandbox with the same location / kind / runtime model as
    /// sandboxes, registered as a Space. With `reuse`, a reachable
    /// registered Space in that location with what was asked for is
    /// returned instead. An existing machine is added with [`Spaces::add`],
    /// not created.
    ///
    /// Cancellable: [`Spaces::cancel_create`] (by `create_id`, the id the
    /// Space will have, or its name) stops it and removes what it made;
    /// dropping the returned future (a closed request) does the same.
    pub async fn create(&self, mut opts: SpaceCreate) -> Result<SpaceCreated> {
        use cua_sandbox_core::progress::{Phase, Progress, report};
        let sink = opts.progress.take().map(|p| p.0);
        let (key, on) = self.create_key(&mut opts)?;
        // Every report names the Space it is about.
        let target = key.id.clone();
        let sink: Option<cua_sandbox_core::progress::Sink> = sink.map(|s| {
            let target = target.clone();
            Arc::new(move |p: &CreateProgress| {
                if p.target.is_empty() && !target.is_empty() {
                    let mut p = p.clone();
                    p.target = target.clone();
                    s(&p)
                } else {
                    s(p)
                }
            }) as cua_sandbox_core::progress::Sink
        });
        if opts.wait == Some(false) && on != On::Cloud {
            // The whole create runs in the background (still cancellable);
            // the caller gets the id it will have. A cloud create returns
            // once its claim is accepted (its errors, such as no credit,
            // reach the caller) and finishes binding in the background.
            let flight = crate::creating::register(self.home_dir(), &key)?;
            let spaces = self.clone();
            let id = key.id.clone();
            opts.wait = Some(true);
            tokio::spawn(cua_sandbox_core::progress::carry(sink, async move {
                match Box::pin(spaces.create_tracked(opts, key, flight)).await {
                    Ok(_) => report(Progress::phase(Phase::Ready)),
                    Err(e) => tracing::warn!(error = %e, "background Space create failed"),
                }
            }));
            return Ok(SpaceCreated::Starting(PendingSpace {
                id,
                phase: "starting",
            }));
        }
        let flight = crate::creating::register(self.home_dir(), &key)?;
        cua_sandbox_core::progress::carry(sink, async move {
            let created = Box::pin(self.create_tracked(opts, key, flight)).await?;
            report(Progress::phase(Phase::Ready));
            Ok(created)
        })
        .await
    }

    /// The GPU options each runtime of `on` offers here: on this machine,
    /// per local runtime (Lume: "GPU acceleration" for macOS VMs; QEMU:
    /// virgl on Linux; containers: NVIDIA on Linux); none on Cua Cloud yet.
    /// A runtime with no option says why ([`cua_sandbox_core::gpu`]).
    pub async fn gpu_support(&self, on: &On) -> Vec<cua_sandbox_core::gpu::GpuSupport> {
        let sbx = &self.inner.sandboxes;
        match on {
            On::Local => sbx.gpu_support(ProviderKind::Local, None).await,
            On::Cloud => sbx.gpu_support(ProviderKind::Fleet, None).await,
            On::Provider(word) => sbx.gpu_support(ProviderKind::Contrib, Some(word)).await,
            _ => vec![],
        }
    }

    /// Names the create before it starts (a generated name is chosen now,
    /// so a cancel and a crash recovery know what to look for) and says
    /// how it is found.
    fn create_key(&self, opts: &mut SpaceCreate) -> Result<(crate::creating::CreateKey, On)> {
        let on = match opts.on.clone() {
            Some(on) => on,
            None => {
                cua_sandbox_core::settings::Settings::load()
                    .and_then(|s| s.default_on())
                    .map_err(|e| Error::invalid(e.to_string()))?
                    .0
            }
        };
        let create_id = opts.create_id.clone().filter(|c| !c.trim().is_empty());
        let generated = opts.name.as_deref().is_none_or(|n| n.trim().is_empty());
        let named = |opts: &mut SpaceCreate| {
            let name = sanitize_label(
                &opts
                    .name
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| format!("space-{}", random_suffix())),
            );
            opts.name = Some(name.clone());
            name
        };
        let key = match &on {
            On::Local => {
                let name = named(opts);
                crate::creating::CreateKey {
                    id: SpaceId::Local { name: name.clone() }.to_string(),
                    stem: name.clone(),
                    name,
                    create_id,
                    generated,
                }
            }
            On::Cloud => {
                let name = named(opts);
                crate::creating::CreateKey {
                    id: SpaceId::Cloud {
                        name: name.clone(),
                        namespace: None,
                    }
                    .to_string(),
                    stem: format!("cloud.{name}"),
                    name,
                    create_id,
                    generated,
                }
            }
            _ => crate::creating::CreateKey {
                id: String::new(),
                name: opts.name.clone().unwrap_or_default(),
                stem: format!("other.{}", random_suffix()),
                create_id,
                generated,
            },
        };
        Ok((key, on))
    }

    /// [`Spaces::create`] once registered: runs it with its journal, and
    /// stops it when it is cancelled (or dropped), removing what it made.
    async fn create_tracked(
        &self,
        opts: SpaceCreate,
        key: crate::creating::CreateKey,
        flight: crate::creating::Flight,
    ) -> Result<SpaceCreated> {
        use crate::creating::{CancelOutcome, CancelState};
        let kind = if key.stem.starts_with("cloud.") {
            "cloud"
        } else if key.stem.starts_with("other.") {
            "host"
        } else {
            "local"
        };
        let expect_env = expects_spacesd(opts.image.as_deref(), opts.spacesd);
        let journal =
            crate::creating::JournalHandle::start(self.home_dir(), &key, kind, expect_env);
        // A create dropped before it returned (a closed request, a Ctrl-C
        // the caller did not handle) is undone the same way.
        let mut guard = UndoOnDrop {
            spaces: self.clone(),
            journal: Some(journal.clone()),
        };
        cua_sandbox_core::progress::report(cua_sandbox_core::progress::Progress::phase(
            cua_sandbox_core::progress::Phase::Preparing,
        ));
        let run = Box::pin(crate::creating::scope(
            journal.clone(),
            Box::pin(self.create_inner(opts)),
        ));
        let result = tokio::select! {
            r = run => r,
            _ = flight.cancelled() => {
                // The create's future is dropped here: every await in it
                // stops, and its drop guards hand off their clean-up.
                cua_sandbox_core::cleanup::settle(Duration::from_secs(90)).await;
                let message = self.undo(&journal.snapshot()).await;
                Err(Error::Cancelled(message))
            }
        };
        guard.journal = None;
        journal.close();
        flight.finish(match &result {
            Err(Error::Cancelled(m)) => CancelOutcome {
                id: key.id.clone(),
                state: CancelState::Cancelled,
                message: m.clone(),
            },
            Ok(_) => CancelOutcome {
                message: format!("{} was already created; delete it to remove it.", key.id),
                id: key.id.clone(),
                state: CancelState::AlreadyCreated,
            },
            Err(e) => CancelOutcome {
                id: key.id.clone(),
                state: CancelState::NotCreating,
                message: format!("The create had already failed: {e}"),
            },
        });
        result
    }

    async fn create_inner(&self, opts: SpaceCreate) -> Result<SpaceCreated> {
        use cua_sandbox_core::placement;
        let (on, source) = match opts.on.clone() {
            Some(on) => (on, cua_sandbox_core::settings::Source::Explicit),
            None => cua_sandbox_core::settings::Settings::load()
                .and_then(|s| s.default_on())
                .map_err(|e| Error::invalid(e.to_string()))?,
        };
        if on == On::Cloud
            && self.inner.fleet.is_none()
            && let Some(hint) = cua_sandbox_core::settings::cloud_default_hint(&source)
        {
            return Err(Error::host(
                cua_spaces_contract::host::FLEET,
                format!(
                    "set CUA_CLIENT_ID/CUA_CLIENT_SECRET (or CUA_TOKEN) to use cloud Spaces; {hint}"
                ),
            ));
        }
        let place = |e: placement::PlacementError| {
            Error::Sandbox(cua_sandbox_core::Error::InvalidPlacement(e))
        };
        if let On::Host(query) = &on {
            if opts.reuse {
                return Err(Error::invalid(
                    "reuse does not apply to host: Spaces; list them with list_spaces",
                ));
            }
            placement::validate(&on, opts.kind, &opts.runtime).map_err(place)?;
            let info = self.create_on_host(query, opts).await?;
            return Ok(SpaceCreated::Ready {
                info,
                reused: false,
            });
        }
        if let On::Provider(word) = &on
            && match self.inner.sandboxes.contrib_provider(word) {
                Ok(p) => p.joins_relay(),
                // A cloud-only word this build lacks says how to get it.
                Err(_) => !cua_sandbox_core::is_contrib_location(word),
            }
        {
            // Your own cloud account (`aws`, `gcp`, `modal`): a sandbox there
            // whose driver joined the relay.
            if opts.reuse {
                return Err(Error::invalid(format!(
                    "reuse does not apply to Spaces in your cloud ({word}); list them with \
                     list_spaces"
                )));
            }
            let info = self.create_in_cloud(word, opts).await?;
            return Ok(SpaceCreated::Ready {
                info,
                reused: false,
            });
        }
        if on.is_existing_machine() {
            return Err(Error::invalid(format!(
                "{on} is an existing machine: add it with add_space (url {}), create_space \
                 makes a new one (on local or cloud)",
                on.to_string().split_once(':').map_or("", |(_, a)| a)
            )));
        }
        placement::validate(&on, opts.kind, &opts.runtime).map_err(place)?;
        let expect_env = expects_spacesd(opts.image.as_deref(), opts.spacesd);
        let provider = match &on {
            On::Local => Provider::Local,
            On::Cloud => Provider::Cloud,
            other => {
                return Err(Error::HostCapabilityMissing {
                    what: format!("provider {}", other.location()),
                    why: "this build has no Spaces backend for it".into(),
                });
            }
        };
        if opts.reuse
            && let Some(info) = self.reusable(provider, &opts.services, expect_env).await?
        {
            return Ok(SpaceCreated::Ready { info, reused: true });
        }
        match on {
            On::Cloud => {
                if opts.gpu.is_some() {
                    return Err(Error::invalid(format!(
                        "gpu: {}",
                        cua_sandbox_core::FLEET_NO_GPU_REASON
                    )));
                }
                if opts.disk_gb.is_some_and(|g| g > 0) {
                    return Err(Error::invalid(
                        "disk_gb sizes local VMs only: Cua Cloud boots the image's disk",
                    ));
                }
                // Refused here, before any pool is written.
                let memory_mb = opts.memory_mb.map(|m| u32::try_from(m).unwrap_or(u32::MAX));
                cua_fleet::check_cloud_size(opts.cpus, memory_mb)
                    .map_err(|e| Error::invalid(e.to_string()))?;
                let runtime = match (&opts.runtime, opts.kind) {
                    (Runtime::Gvisor, _) | (Runtime::Auto, Kind::Container) => {
                        Some(FleetRuntime::Gvisor)
                    }
                    (Runtime::Kubevirt, _) | (Runtime::Auto, Kind::Vm) => {
                        Some(FleetRuntime::Kubevirt)
                    }
                    _ => None,
                };
                let claim = FleetClaim {
                    image: opts.image,
                    runtime,
                    name: opts.name,
                    wait: opts.wait,
                    env_token: opts.env_token,
                    command: opts.command,
                    env: opts.env,
                    services: opts.services,
                    spacesd: opts.spacesd,
                    cpus: opts.cpus,
                    memory_mb,
                };
                Ok(match self.claim_fleet(claim).await? {
                    Ok(info) => SpaceCreated::Ready {
                        info,
                        reused: false,
                    },
                    Err(p) => SpaceCreated::Starting(p),
                })
            }
            _ => {
                let name = sanitize_label(
                    &opts
                        .name
                        .unwrap_or_else(|| format!("space-{}", random_suffix())),
                );
                let local = LocalProvision {
                    image: opts.image,
                    kind: opts.kind,
                    runtime: opts.runtime,
                    name: Some(name.clone()),
                    cpus: opts.cpus,
                    memory_mb: opts.memory_mb,
                    disk_gb: opts.disk_gb,
                    timeout: opts.timeout,
                    command: opts.command,
                    env: opts.env,
                    services: opts.services,
                    spacesd: opts.spacesd,
                    gpu: opts.gpu,
                };
                // `create` runs a `wait = false` create in the background
                // itself, so here it always waits.
                Ok(SpaceCreated::Ready {
                    info: self.provision_local(local).await?,
                    reused: false,
                })
            }
        }
    }

    /// The warm pool Spaces claims `image` on `runtime` from:
    /// `<namespace>-<runtime>-<sha256(image)[..8]>`, a DNS label.
    pub fn fleet_pool_name(&self, runtime: FleetRuntime, image: &str) -> String {
        self.pool_name(runtime, image)
    }

    fn pool_name(&self, runtime: FleetRuntime, image: &str) -> String {
        use sha2::Digest;
        let digest = hex::encode(sha2::Sha256::digest(image.as_bytes()));
        let suffix = format!("-{}-{}", runtime.as_str(), &digest[..8]);
        let base = &self.inner.namespace;
        let keep = 63usize.saturating_sub(suffix.len()).min(base.len());
        sanitize_label(&format!("{}{suffix}", base[..keep].trim_end_matches('-')))
    }

    /// Provisions a Space on the local runtime and registers it. Readiness
    /// is the runtime's plus a TCP probe per declared service (the Spaces
    /// images: the spacesd port). An image without cua-spacesd
    /// becomes a Space of its declared services within the handshake
    /// timeout, never after the whole budget.
    pub(crate) async fn provision_local(&self, opts: LocalProvision) -> Result<SpaceInfo> {
        if !self.inner.has_local {
            // A custom Sandboxes may still carry a local runtime; let it say.
            tracing::debug!(
                "no local runtime passed to SpacesBuilder; using the sandbox manager's"
            );
        }
        let name = sanitize_label(
            &opts
                .name
                .clone()
                .unwrap_or_else(|| format!("space-{}", random_suffix())),
        );
        let expect_env = expects_spacesd(opts.image.as_deref(), opts.spacesd);
        // An alias (`linux`, `macos:26`, `macos:26-slim`) names a canonical
        // image, as it does for a cloud or host Space; the local runtime
        // only knows registry references (it would try to pull `linux`).
        let image = opts
            .image
            .as_deref()
            .map(|i| cua_image::canonical::alias(i).unwrap_or_else(|| i.to_string()))
            .unwrap_or_else(|| cua_fleet::canonical_image("linux"));
        let token = format!("{:032x}", rand::random::<u128>());
        // Until the Space is registered, the create's journal holds its
        // token and what it made: a process that dies mid-create (a daemon
        // crash, a quit) leaves it, and [`Spaces::recover_interrupted_creates`]
        // then registers the sandbox or deletes it, never a hidden one; a
        // cancel removes what it made. Only a name nothing used before is
        // this create's to delete.
        let fresh = !self.inner.sandboxes.local_name_in_use(&name).await;
        crate::creating::set_token(&token);
        crate::creating::record(crate::creating::Made::LocalSandbox {
            name: name.clone(),
            fresh,
        });
        let timeout = opts.timeout.unwrap_or(Duration::from_secs(600));
        let mut create = CreateOptions::new(ProviderKind::Local, &image).name(&name);
        create.kind = opts.kind;
        create.runtime = opts.runtime.clone();
        create.cpus = opts.cpus.unwrap_or(2);
        create.memory_mb = opts.memory_mb.unwrap_or(4096);
        create.disk_gb = opts.disk_gb.filter(|g| *g > 0);
        create.env = opts.env.clone();
        create.env.insert("CUA_ENV_TOKEN".into(), token.clone());
        create.env_token = Some(token.clone());
        create.ready_timeout = timeout;
        create.ready_timeout_given = opts.timeout.is_some();
        create.command = opts.command.clone().filter(|c| !c.is_empty());
        create.gpu = opts.gpu.clone().filter(|g| !g.trim().is_empty());
        for (svc, port) in opts.services.iter().filter(|(s, _)| s.as_str() != "env") {
            create = create
                .service(svc.clone(), *port)
                .wait_for(Probe::Tcp(*port));
        }
        if expect_env {
            create = create.wait_for(Probe::Tcp(cua_proto::SPACESD_DEFAULT_PORT));
        }
        let started = tokio::time::Instant::now();
        // Cancel-safe: dropped (a cancelled create), it stops and deletes
        // what it made (its own token never fires; the create's does).
        let created = self
            .inner
            .sandboxes
            .create_cancellable(create, cua_sandbox_core::CancellationToken::new())
            .await;
        if let Err(e) = created {
            // A start that failed after the instance existed (a readiness
            // probe that never passed) leaves a running VM no registry
            // lists: nothing would ever show or stop it. One under a name
            // the caller chose stays to debug.
            if fresh && crate::creating::generated_name() {
                match self.inner.sandboxes.delete(&name).await {
                    Ok(()) | Err(cua_sandbox_core::Error::NotFound(_)) => {}
                    Err(d) => {
                        tracing::warn!(sandbox = %name, error = %d, "could not delete a Space that failed to start")
                    }
                }
            }
            return Err(e.into());
        }
        let id = SpaceId::Local { name: name.clone() };
        let credential = Credential {
            url: None,
            token: Some(token),
            ..Default::default()
        };
        // A chosen timeout governs the whole wait; the default budget caps
        // the handshake at SPACESD_READY_TIMEOUT.
        let mut remaining = timeout
            .saturating_sub(started.elapsed())
            .max(Duration::from_secs(10));
        if opts.timeout.is_none() {
            remaining = remaining.min(cua_sandbox_core::SPACESD_READY_TIMEOUT);
        }
        cua_sandbox_core::progress::report(cua_sandbox_core::progress::Progress::phase(
            cua_sandbox_core::progress::Phase::Connecting,
        ));
        let space = match self
            .connect_when_ready(&id, &credential, remaining, expect_env)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                if let Ok(sb) = self.inner.sandboxes.connect(&name).await {
                    let _ = sb.delete().await;
                }
                return Err(e);
            }
        };
        self.store(&space, credential)?;
        self.inner
            .connections
            .lock()
            .await
            .insert(id.to_string(), space);
        #[cfg(feature = "mcp")]
        self.notify_connected(&id);
        self.info(&id)
    }

    /// Local creates a process that died left half done (its daemon
    /// crashed or was killed between starting the sandbox and registering
    /// the Space): each such sandbox is registered when its cua-spacesd
    /// answers within `budget`, else deleted, so nothing keeps running
    /// that no Space lists. Creates of live processes are left alone. The
    /// daemon runs this when it starts.
    ///
    /// A create someone cancelled (its `.cancel` marker), a cloud or host
    /// create, and one under a name that was in use before are not
    /// registered: what they made is removed ([`Spaces::cancel_create`]'s
    /// undo), and a sandbox that existed before is left as it was.
    pub async fn recover_interrupted_creates(&self, budget: Duration) -> Vec<RecoveredCreate> {
        use crate::creating::Made;
        let mut out = Vec::new();
        let home = self.home_dir().to_path_buf();
        for (stem, j) in crate::creating::journals(&home)
            .into_iter()
            .filter(|(_, j)| crate::creating::gone(j))
        {
            let local_made = j.made.iter().find_map(|m| match m {
                Made::LocalSandbox { fresh, .. } => Some(*fresh),
                _ => None,
            });
            if crate::creating::cancel_marked(&home, &stem) || j.kind != "local" {
                let message = self.undo(&j).await;
                tracing::info!(create = %stem, message, "interrupted create undone");
                crate::creating::remove(&home, &stem);
                out.push(RecoveredCreate {
                    id: if j.id.is_empty() {
                        j.name.clone()
                    } else {
                        j.id.clone()
                    },
                    outcome: RecoveryOutcome::Deleted(message),
                });
                continue;
            }
            let id = SpaceId::Local {
                name: j.name.clone(),
            };
            // Nothing made yet (cut off while resolving the image), or a
            // name that was someone else's: nothing of this create's to
            // delete.
            let untouchable =
                (j.token.is_empty() && j.made.is_empty()) || local_made == Some(false);
            let outcome = if self
                .inner
                .registry
                .get(&id.to_string())
                .ok()
                .flatten()
                .is_some()
            {
                RecoveryOutcome::AlreadyRegistered
            } else if untouchable && self.inner.sandboxes.connect(&j.name).await.is_err() {
                RecoveryOutcome::NothingLeft
            } else if self.inner.sandboxes.connect(&j.name).await.is_err() {
                // Cut off before the sandbox was recorded: an instance the
                // runtime started is deleted, never left running unlisted.
                match self.inner.sandboxes.delete_local_instance(&j.name).await {
                    Ok(true) => RecoveryOutcome::Deleted(
                        "the create was cut off before the sandbox was recorded".into(),
                    ),
                    Ok(false) => RecoveryOutcome::NothingLeft,
                    Err(e) => RecoveryOutcome::Failed(format!("delete: {e}")),
                }
            } else {
                let credential = Credential {
                    token: Some(j.token.clone()),
                    ..Default::default()
                };
                match self
                    .connect_when_ready(&id, &credential, budget, j.spacesd)
                    .await
                {
                    Ok(space) => match self.store(&space, credential) {
                        Ok(()) => {
                            self.inner
                                .connections
                                .lock()
                                .await
                                .insert(id.to_string(), space);
                            #[cfg(feature = "mcp")]
                            self.notify_connected(&id);
                            RecoveryOutcome::Registered
                        }
                        Err(e) if untouchable => RecoveryOutcome::Failed(e.to_string()),
                        Err(e) => self.discard_interrupted(&j.name, e.to_string()).await,
                    },
                    Err(e) if untouchable => RecoveryOutcome::Failed(e.to_string()),
                    Err(e) => self.discard_interrupted(&j.name, e.to_string()).await,
                }
            };
            tracing::info!(space = %id, ?outcome, "interrupted create");
            crate::creating::remove(&home, &stem);
            out.push(RecoveredCreate {
                id: id.to_string(),
                outcome,
            });
        }
        out
    }

    async fn discard_interrupted(&self, name: &str, why: String) -> RecoveryOutcome {
        let deleted = match self.inner.sandboxes.connect(name).await {
            Ok(sb) => sb.delete().await.map(|_| ()).map_err(Error::from),
            Err(_) => self
                .inner
                .sandboxes
                .delete_local_instance(name)
                .await
                .map(|_| ())
                .map_err(Error::from),
        };
        match deleted {
            Ok(()) => RecoveryOutcome::Deleted(why),
            Err(e) => RecoveryOutcome::Failed(format!("{why}; delete: {e}")),
        }
    }

    /// Deletes a Space's sandbox and forgets it: a cloud Space's sandbox is
    /// deleted (metering stops), a local one's instance is deleted. A Space
    /// added by address (`direct:`, `relay:`) is only forgotten: cua did not
    /// create it. Hotspots on it stop.
    pub async fn delete(&self, space: &str) -> Result<String> {
        let id = self.resolve(space)?;
        self.drop_connection(&id).await;
        self.inner.thumbnails.remove(&id.to_string());
        // A Space in your cloud: its sandbox goes, with everything the
        // sandbox layer made for it (instance, relay machine).
        if let Some(sandbox) = self.cloud_sandbox_of(&id.to_string())? {
            self.inner.sandboxes.delete(&sandbox).await?;
            let _ = self.inner.registry.remove(&id.to_string());
            let _ = self.relay_machines().await;
            return Ok(format!("Deleted {id} (sandbox {sandbox} in your cloud)."));
        }
        let what = match &id {
            SpaceId::Cloud { name, namespace } => {
                let fleet = self.fleet_client()?;
                let namespace = match namespace.clone() {
                    Some(ns) => ns,
                    None => match self
                        .inner
                        .registry
                        .credential(&id.to_string())?
                        .and_then(|c| c.namespace)
                    {
                        Some(ns) => ns,
                        None => cloud_namespace(fleet, name).await?,
                    },
                };
                fleet.release(&namespace, name).await?;
                format!("Deleted {id} (cloud sandbox {name}).")
            }
            SpaceId::Local { name } => {
                // By name: a stopped or unreachable Space deletes too (a
                // handle needs it running), with no connect round trip.
                match self.inner.sandboxes.delete(name).await {
                    Ok(()) | Err(cua_sandbox_core::Error::NotFound(_)) => {}
                    Err(e) => return Err(e.into()),
                }
                // A Space this machine provides to another device leaves
                // the relay and the host's list too (audited).
                crate::host_spaces::forget_provided(self, &id.to_string()).await;
                format!("Deleted {id} (local sandbox {name}).")
            }
            SpaceId::Direct { .. } => {
                // A Space a direct host created (`on="host:<name>"`): that
                // host deletes it.
                if let Some((host, on_host)) =
                    self.inner.registry.direct_host_of(&id.to_string())?
                {
                    let message = self
                        .delete_on_direct_host_space(&id, &host, &on_host)
                        .await?;
                    self.inner.registry.remove(&id.to_string())?;
                    return Ok(message);
                }
                let _ = self.inner.registry.forget_direct(&id.to_string());
                format!(
                    "Removed {id} (added by address, so only forgotten; the machine is untouched)."
                )
            }
            SpaceId::Relay { machine_id } => {
                // A Space one of your machines provides is deleted there.
                let row = self.relay_row(machine_id).await;
                // A Space in someone's own cloud that another device created:
                // that device deletes it (asked through the relay), or the
                // refusal says where.
                if let Some(m) = row.as_ref().filter(|m| crate::cloud::is_cloud_machine(m)) {
                    let message = self.delete_cloud_elsewhere(m).await?;
                    let _ = self.inner.registry.remove(&id.to_string());
                    let _ = self.relay_machines().await;
                    return Ok(message);
                }
                match row.as_ref().and_then(|m| m.host.clone().map(|h| (m, h))) {
                    Some((m, host)) => {
                        let message = self.delete_on_host(m, &host).await?;
                        let _ = self.inner.registry.remove(&id.to_string());
                        let _ = self.relay_machines().await;
                        return Ok(message);
                    }
                    None => format!(
                        "Removed {id} (disconnected; the machine stays in your relay directory)."
                    ),
                }
            }
        };
        self.inner.registry.remove(&id.to_string())?;
        Ok(what)
    }

    /// The relay directory row of `machine_id` (cached, else refreshed);
    /// `None` without a relay account or when the relay does not list it.
    async fn relay_row(&self, machine_id: &str) -> Option<crate::relay::RelayMachine> {
        self.relay_account()?;
        let cached = self
            .inner
            .relay_cache
            .lock()
            .expect("relay cache")
            .iter()
            .find(|m| m.id == machine_id)
            .cloned();
        match cached {
            Some(m) => Some(m),
            None => self
                .relay_machines()
                .await
                .ok()
                .and_then(|all| all.into_iter().find(|m| m.id == machine_id)),
        }
    }

    // ---------------------------------------------------------- thumbnails

    /// The thumbnail cache every client of this runtime shares.
    pub fn thumbnails(&self) -> &crate::thumbnails::ThumbnailCache {
        &self.inner.thumbnails
    }

    /// The Space's latest thumbnail (a small JPEG of its primary display):
    /// the cached one when it is younger than `max_age` (none: any age),
    /// else a fresh capture through cua-spacesd, kept for the next caller.
    /// When the capture fails, the older cached one (its `captured_at`
    /// says how old); with nothing cached, the capture's error. Asking
    /// keeps [`Self::refresh_thumbnails`] going ([`crate::thumbnails`]).
    pub async fn thumbnail(
        &self,
        space: &str,
        max_age: Option<Duration>,
    ) -> Result<crate::thumbnails::Thumbnail> {
        let id = self.resolve(space)?.to_string();
        let cache = &self.inner.thumbnails;
        cache.note_interest(std::time::Instant::now());
        let cached = cache.get(&id);
        if let Some(t) = &cached
            && t.is_fresh(max_age, SystemTime::now())
        {
            return Ok(t.clone());
        }
        match self.capture_thumbnail(&id).await {
            Ok(t) => Ok(t),
            Err(e) => cached.ok_or(e),
        }
    }

    /// Captures and caches the Space's thumbnail now.
    async fn capture_thumbnail(&self, id: &str) -> Result<crate::thumbnails::Thumbnail> {
        use crate::thumbnails::{MAX_DIMENSION, QUALITY, Thumbnail};
        let space = self.space(id).await?;
        let shot = space
            .spacesd()?
            .screenshot(cua_spacesd_client::ScreenshotOptions {
                display: None,
                format: pb::ImageFormat::Jpeg,
                quality: QUALITY,
                max_dimension: MAX_DIMENSION,
                include_cursor: false,
            })
            .await?;
        let t = Thumbnail {
            image: shot.image.to_vec(),
            format: match shot.format {
                pb::ImageFormat::Png => "png",
                pb::ImageFormat::Webp => "webp",
                _ => "jpeg",
            }
            .into(),
            width: shot.width,
            height: shot.height,
            captured_at: SystemTime::now(),
        };
        self.inner.thumbnails.put(id, t.clone());
        Ok(t)
    }

    /// One background pass over the thumbnails, while someone asked for
    /// one recently: forgets those of Spaces that are gone, then captures
    /// each running Space's that is due (older than
    /// [`crate::thumbnails::BACKGROUND_INTERVAL`]). Returns how many it
    /// captured. A Space that does not answer is skipped quietly.
    pub async fn refresh_thumbnails(&self) -> usize {
        let cache = &self.inner.thumbnails;
        if !cache.interested(std::time::Instant::now()) {
            return 0;
        }
        let Ok(spaces) = self.list() else { return 0 };
        cache.retain(&spaces.iter().map(|s| s.id.clone()).collect());
        let now = SystemTime::now();
        let mut captured = 0;
        for info in spaces {
            let off = matches!(info.power_state.as_str(), "suspended" | "stopped");
            if off || info.spacesd_version.is_empty() || !cache.due(&info.id, now) {
                continue;
            }
            match tokio::time::timeout(Duration::from_secs(10), self.capture_thumbnail(&info.id))
                .await
            {
                Ok(Ok(_)) => captured += 1,
                Ok(Err(e)) => tracing::debug!(space = %info.id, error = %e, "thumbnail skipped"),
                Err(_) => tracing::debug!(space = %info.id, "thumbnail timed out"),
            }
        }
        captured
    }

    /// Runs [`Self::refresh_thumbnails`] every
    /// [`crate::thumbnails::REFRESH_TICK`] until the runtime is dropped
    /// (the daemon starts it once).
    pub fn spawn_thumbnail_refresh(&self) -> tokio::task::JoinHandle<()> {
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(crate::thumbnails::REFRESH_TICK).await;
                let Some(inner) = weak.upgrade() else { return };
                Spaces { inner }.refresh_thumbnails().await;
            }
        })
    }

    // --------------------------------------------------------------- power

    /// [`SpaceInfo::power`] and [`SpaceInfo::power_state`] of the Space
    /// `id`, from state files only: empty for a Space that cannot be turned
    /// off and on. A Space one of your machines provides says so in its
    /// relay row instead ([`relay_info`]).
    pub fn power_of(&self, id: &str) -> (String, String) {
        let Ok(SpaceId::Local { name }) = SpaceId::parse(id) else {
            return Default::default();
        };
        self.sandbox_power(&name)
    }

    /// [`Self::power_of`] for the sandbox `name` (a local Space's, or the
    /// cloud sandbox behind a `relay:` Space).
    fn sandbox_power(&self, name: &str) -> (String, String) {
        let sandboxes = &self.inner.sandboxes;
        match sandboxes.power_control(name) {
            Some(control) => (
                control.as_str().into(),
                sandboxes
                    .power_state(name)
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default(),
            ),
            None => Default::default(),
        }
    }

    /// Turns a Space off the way its provider can ([`SpaceInfo::power`]):
    /// suspends it, keeping its memory (a container, a QEMU VM), or stops
    /// it, keeping its disk (a Lume VM, a Space one of your machines
    /// provides, which that machine stops; a Space in your own cloud, whose
    /// instance stops). Streams and hotspots on it end first. A Fleet cloud
    /// Space, a Space added by address and your own computers on the relay
    /// cannot be turned off.
    pub async fn stop(&self, space: &str) -> Result<SpacePower> {
        self.set_power(space, false).await
    }

    /// Turns a Space on again: resumes a suspended one, boots a stopped one
    /// (and, for a Space one of your machines provides, attaches it to the
    /// relay again), and leaves a running one as it is. Returns once it
    /// answers again (bounded).
    pub async fn start(&self, space: &str) -> Result<SpacePower> {
        self.set_power(space, true).await
    }

    async fn set_power(&self, space: &str, on: bool) -> Result<SpacePower> {
        let id = self.resolve(space)?;
        let cloud = self.cloud_sandbox_of(&id.to_string())?;
        let refuse = |provider: &str| Error::WrongProvider {
            op: if on { "Turning on" } else { "Turning off" }.into(),
            provider: provider.into(),
        };
        match &id {
            SpaceId::Local { name } => {
                let sandboxes = &self.inner.sandboxes;
                // Nothing keeps a connection into a Space that is going
                // away, nor one cached from before it was turned off.
                self.drop_connection(&id).await;
                let state = if on {
                    sandboxes.power_on(name).await?;
                    cua_sandbox_core::PowerState::Running
                } else {
                    sandboxes.power_off(name).await?
                };
                let control = sandboxes
                    .power_control(name)
                    .unwrap_or(cua_sandbox_core::PowerControl::Stop);
                Ok(SpacePower::new(&id.to_string(), control, state))
            }
            // A Space in your own cloud: its sandbox stops and starts
            // there (compute billing stops; its disk stays), and its
            // relay machine comes back when it boots.
            SpaceId::Relay { .. } if cloud.is_some() => {
                let sandbox = cloud.unwrap_or_default();
                let sandboxes = &self.inner.sandboxes;
                let Some(control) = sandboxes.power_control(&sandbox) else {
                    return Err(refuse(
                        "your cloud (this platform cannot stop a sandbox; delete it instead)",
                    ));
                };
                self.drop_connection(&id).await;
                let state = if on {
                    sandboxes.power_on(&sandbox).await?;
                    let _ = self.relay_machines().await;
                    cua_sandbox_core::PowerState::Running
                } else {
                    sandboxes.power_off(&sandbox).await?
                };
                Ok(SpacePower::new(&id.to_string(), control, state))
            }
            SpaceId::Relay { machine_id } => {
                let row = self.relay_row(machine_id).await;
                match row.and_then(|m| {
                    let host = m.host.clone().filter(|h| !h.is_empty())?;
                    Some((m, host))
                }) {
                    Some((m, host)) => {
                        self.drop_connection(&id).await;
                        self.power_on_host(&m, &host, on).await
                    }
                    None => Err(refuse("relay (only a Space one of your machines provides)")),
                }
            }
            SpaceId::Cloud { .. } => Err(refuse("cloud")),
            // A Space a direct host created: that host turns it off or on.
            SpaceId::Direct { .. } => match self.inner.registry.direct_host_of(&id.to_string())? {
                Some((host, on_host)) => {
                    self.drop_connection(&id).await;
                    self.power_on_direct_host(&id, &host, &on_host, on).await
                }
                None => Err(refuse(
                    "direct (only a Space one of your direct hosts created)",
                )),
            },
        }
    }

    /// Where `download` lands by default.
    pub fn download_dir(&self) -> &Path {
        &self.inner.download_dir
    }
}

/// Errors from the spacesd handshake that mean "this URL is not an
/// spacesd" (so it may be another service), as opposed to failures to
/// report as they are.
fn not_an_spacesd(e: &Error) -> bool {
    matches!(
        e,
        Error::SpacesdNotAvailable { .. }
            | Error::Env(cua_spacesd_client::Error::Unauthenticated(_))
            | Error::Env(cua_spacesd_client::Error::Transport(_))
    )
}

/// `container:ghcr.io/x:1` -> `ghcr.io/x:1` (the local runtime prefixes).
/// What [`Spaces::recover_interrupted_creates`] did with one create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredCreate {
    /// `local:<name>`.
    pub id: String,
    /// What happened to it.
    pub outcome: RecoveryOutcome,
}

/// The fate of an interrupted create.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// Its cua-spacesd answered: the Space is registered and listed.
    Registered,
    /// It never became a Space (why): its sandbox was deleted.
    Deleted(String),
    /// Its sandbox could not be deleted (why).
    Failed(String),
    /// It had been registered after all.
    AlreadyRegistered,
    /// No sandbox was started before the process died.
    NothingLeft,
}

/// Undoes a create whose future was dropped before it returned (a closed
/// request, a Ctrl-C the caller did not handle): the same clean-up as a
/// cancel, in the background.
struct UndoOnDrop {
    spaces: Spaces,
    journal: Option<crate::creating::JournalHandle>,
}

impl Drop for UndoOnDrop {
    fn drop(&mut self) {
        let Some(journal) = self.journal.take() else {
            return;
        };
        let spaces = self.spaces.clone();
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                cua_sandbox_core::cleanup::settle(Duration::from_secs(90)).await;
                let message = spaces.undo(&journal.snapshot()).await;
                journal.close();
                tracing::info!(message, "a dropped create was undone");
            });
        }
    }
}

fn strip_runtime_prefix(image: &str) -> &str {
    for prefix in ["container:", "docker:", "vm:", "disk:", "lume:"] {
        if let Some(rest) = image.strip_prefix(prefix) {
            return rest;
        }
    }
    image
}

/// Whether `image` is expected to run cua-spacesd: the Spaces images
/// (and no image, which means the default one) do; any other image does
/// not unless `explicit` says so. Only decides how long provisioning waits
/// for the driver after the sandbox is ready.
pub fn expects_spacesd(image: Option<&str>, explicit: Option<bool>) -> bool {
    if let Some(e) = explicit {
        return e;
    }
    match image.map(str::trim).filter(|i| !i.is_empty()) {
        // The default Space image is the canonical Linux image.
        None => true,
        // The canonical images (and their CUA_IMAGE_<OS> overrides) carry
        // cua-spacesd; anything else says so with `spacesd`. A local runtime
        // prefix (`container:`, `vm:`, ...) picks the variant, not the image.
        Some(image) => cua_fleet::is_canonical_image(strip_runtime_prefix(image)),
    }
}

/// Splits an MCP endpoint URL into its origin (the service base) and path.
fn split_mcp_url(url: &str) -> Result<(String, String)> {
    let u = url::Url::parse(url).map_err(|e| Error::invalid(format!("bad URL {url:?}: {e}")))?;
    let mut path = u.path().to_string();
    if let Some(q) = u.query() {
        path = format!("{path}?{q}");
    }
    Ok((u.origin().ascii_serialization(), path))
}

/// The key a Fleet pool is named after (see [`Spaces::fleet_pool_name`]):
/// the image alone for a plain Spaces claim (unchanged pool names), else the
/// image plus what else shapes the template (`command`, `env` and the
/// services). A sized Space's key is [`sized_pool_key`].
pub fn pool_key(
    image: &str,
    command: Option<&[String]>,
    env: &BTreeMap<String, String>,
    services: &BTreeMap<String, u16>,
) -> String {
    sized_pool_key(image, command, env, services, None, None)
}

/// [`pool_key`] plus the size (`vmTemplate.cpuCores`, `vmTemplate.memory`
/// in MiB), which shapes the template too. Unsized, it is [`pool_key`].
pub fn sized_pool_key(
    image: &str,
    command: Option<&[String]>,
    env: &BTreeMap<String, String>,
    services: &BTreeMap<String, u16>,
    cpus: Option<u32>,
    memory_mb: Option<u32>,
) -> String {
    if command.is_none()
        && env.is_empty()
        && services.is_empty()
        && cpus.is_none()
        && memory_mb.is_none()
    {
        return image.to_string();
    }
    let mut key = serde_json::json!({"command": command, "env": env, "services": services});
    // Only when set, so the pools of unsized Spaces keep their names.
    if let Some(c) = cpus {
        key["cpu"] = c.into();
    }
    if let Some(m) = memory_mb {
        key["memory_mb"] = m.into();
    }
    format!("{image}\n{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pinned_cloud_image_carries_its_digest_a_tag_none() {
        let d = format!("sha256:{}", "a".repeat(64));
        assert_eq!(
            image_with_pinned_digest(format!("ghcr.io/trycua/linux@{d}")),
            (format!("ghcr.io/trycua/linux@{d}"), d.clone())
        );
        assert_eq!(
            image_with_pinned_digest("ghcr.io/trycua/linux:24.04".into()),
            ("ghcr.io/trycua/linux:24.04".into(), String::new())
        );
        assert_eq!(
            image_with_pinned_digest("repo@sha256:".into()).1,
            String::new()
        );
    }

    #[test]
    fn a_runtime_prefix_does_not_hide_a_canonical_image() {
        assert!(expects_spacesd(Some("ghcr.io/trycua/linux:24.04"), None));
        assert!(expects_spacesd(
            Some("container:ghcr.io/trycua/linux:24.04"),
            None
        ));
        assert!(expects_spacesd(
            Some("vm:ghcr.io/trycua/linux:24.04-disk"),
            None
        ));
        assert!(!expects_spacesd(Some("container:python:3.12-slim"), None));
        assert!(!expects_spacesd(
            Some("container:ghcr.io/trycua/linux:24.04"),
            Some(false)
        ));
    }

    #[test]
    fn labels_are_dns_safe() {
        assert_eq!(sanitize_label("Client_ID.42"), "client-id-42");
        assert_eq!(sanitize_label("--x--"), "x");
        assert_eq!(sanitize_label(&"a".repeat(80)).len(), 63);
    }

    #[test]
    fn pool_names_fit_and_differ_by_image_and_runtime() {
        let spaces = Spaces::builder()
            .home(tempfile::tempdir().unwrap().keep())
            .fleet_namespace("cua-spaces-".to_string() + &"x".repeat(70))
            .build();
        let a = spaces.pool_name(FleetRuntime::Kubevirt, "img:1");
        let b = spaces.pool_name(FleetRuntime::Kubevirt, "img:2");
        let c = spaces.pool_name(FleetRuntime::Gvisor, "img:docker-1");
        assert!(a.len() <= 63 && b.len() <= 63 && c.len() <= 63);
        assert_ne!(a, b);
        assert!(c.contains("-gvisor-"));
    }
}
