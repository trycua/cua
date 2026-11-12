//! [`Spaces`]: the registry, the two ways to get a Space ([`Spaces::create`]
//! a new sandbox where `on` says, or [`Spaces::add`] an existing machine by
//! address), and connected [`Space`] handles. [`Spaces::delete`] undoes a
//! create; [`Spaces::remove`] forgets a Space without touching it.

use crate::error::{Error, Result};
use crate::id::{Provider, SpaceId, authority};
use crate::operator::{ControlServerDisplay, OperatorDisplay};
use crate::registry::{Credential, Registry, cua_home};
use crate::space::{Gateway, Space, SpaceInfo, SpacePower, SpaceResize};
use crate::space::{ServiceSource, SpaceService};
use cua_proto::daemon::v1 as dpb;
use cua_sandbox_core::placement::{Kind, On, Runtime};
use cua_sandbox_core::{CreateOptions, LocalRuntime, PortTarget, Probe, ProviderKind, Sandboxes};
use cua_spacesd_client::{ConnectOptions, SpacesdClient, pb};
use cua_vmm::stack::boxed;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

/// Default time allowed for the `GetCapabilities` handshake.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Options for [`Spaces::create`]: the same location / kind / runtime model
/// as sandboxes ([`cua_sandbox_core::placement`]).
#[derive(Clone, Default)]
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
    /// Name. Default a friendly `adjective-animal` (`brave-otter`).
    pub name: Option<String>,
    /// vCPUs. Local default 2.
    pub cpus: Option<u32>,
    /// Memory in MiB. Local default 4096.
    pub memory_mb: Option<u64>,
    /// Grow the VM's disk to this many GiB (local VMs; `None` keeps the
    /// image's size). Containers and provider Spaces refuse it.
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
    /// Keep it on your machines (`on` [`On::Auto`]): created again on
    /// another one, with the same name, when its machine goes away
    /// ([`crate::durable`]). Needs the cua daemon of Cua Spaces.
    pub durable: bool,
    /// Directories of this machine mounted into the Space (`on` local
    /// only): a container binds them, a macOS VM shares them (under
    /// `/Volumes/My Shared Files/<name>`, see
    /// [`cua_vmm::lume::macos_share_path`]). Kinds that cannot mount refuse
    /// them. A cluster job's Space gets its job directory this way.
    pub mounts: Vec<cua_vmm::Mount>,
    /// No desktop: only cua-spacesd runs (the one headless switch,
    /// [`cua_sandbox_core::CreateOptions::headless`]).
    pub headless: bool,
    /// A fork source (a "bake" Space): a local QEMU VM's guest RAM is a
    /// shared file, so its checkpoints take milliseconds and forks of them
    /// share its pages. Other kinds (and cloud Spaces) ignore it.
    pub forkable: bool,
    /// Named volumes of Cua Volume mounted in the Space
    /// ([`crate::named_volumes`]). Needs the Cua Volume extension.
    pub volumes: Vec<crate::named_volumes::VolumeMount>,
    /// Guest network ([`cua_sandbox_core::NetworkMode::None`]: no outbound
    /// network; QEMU VMs on this machine or one of your machines; not with
    /// `durable` or `reuse`). A machine whose cua cannot enforce it is
    /// refused, never given an open Space.
    pub network: cua_sandbox_core::NetworkMode,
    /// Labels kept with the Space ([`SpaceInfo::labels`]; see
    /// [`crate::registry::validate_labels`] for the limits). Applies on
    /// every create path; a durable Space takes none.
    pub labels: BTreeMap<String, String>,
    /// Egress policy ([`crate::egress`]), enforced by the host the Space
    /// runs on, outside the guest, from its first instruction. A kind or
    /// location with no host-side point refuses it before anything is
    /// created (see [`crate::egress`]); not with `durable`, `reuse` or a
    /// create that does not wait.
    pub egress: crate::egress::EgressPolicy,
    /// Network settings ([`crate::network`]): a preset and the user's
    /// lists. `None`: what `network` and `egress` say (`internet` when
    /// neither says anything).
    pub network_settings: Option<crate::network::NetworkSettings>,
    /// Create it as a child of this Space (id or name): its network is no
    /// more open than its parent's, and it joins the parent's tree.
    pub parent: Option<String>,
    /// The tree of another computer this Space joins ([`crate::overlay`]):
    /// set by a computer that creates a child on one of your other computers,
    /// so the new Space is a member of the same tree there. The id is
    /// qualified with the computer the tree started on.
    pub tree: Option<String>,
    /// With `parent`: let its network be more open than its parent's
    /// (recorded on the Space as who widened it and when).
    pub widen: bool,
    /// On a linked cloud (`on` `aws`, `hetzner`, ...): the machine type its
    /// host must be (`c7g.xlarge`, `cax41`): a pin in the same elastic
    /// solve. `None`: the solver picks.
    pub instance_type: Option<String>,
    /// On a linked cloud: the region its host must be in. `None`: any the
    /// cloud offers.
    pub region: Option<String>,
    /// `Some(false)`: keep the Space off the owner's account, so the phone
    /// and the owner's other computers never see it (`--no-relay`; the
    /// label `cua.relay=off` says the same on every path, the daemon's
    /// included). `None` and `Some(true)`: it is listed while this computer
    /// is signed in and sharing (the `relay.spaces` setting), and a child
    /// follows its parent's `off`.
    pub relay: Option<bool>,
}

impl std::fmt::Debug for SpaceCreate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpaceCreate")
            .field("image", &self.image)
            .field("on", &self.on)
            .field("kind", &self.kind)
            .field("runtime", &self.runtime)
            .field("name", &self.name)
            .field("cpus", &self.cpus)
            .field("memory_mb", &self.memory_mb)
            .field("disk_gb", &self.disk_gb)
            .field("timeout", &self.timeout)
            .field("wait", &self.wait)
            .field("reuse", &self.reuse)
            .field("command", &self.command)
            .field("env", &self.env)
            .field("services", &self.services)
            .field("spacesd", &self.spacesd)
            .field("env_token", &self.env_token.as_ref().map(|_| "<redacted>"))
            .field("progress", &self.progress)
            .field("create_id", &self.create_id)
            .field("gpu", &self.gpu)
            .field("durable", &self.durable)
            .field("mounts", &self.mounts)
            .field("headless", &self.headless)
            .field("forkable", &self.forkable)
            .field("volumes", &self.volumes)
            .field("network", &self.network)
            .field("labels", &self.labels)
            .field("egress", &self.egress)
            .field("network_settings", &self.network_settings)
            .field("parent", &self.parent)
            .field("tree", &self.tree)
            .field("widen", &self.widen)
            .field("instance_type", &self.instance_type)
            .field("region", &self.region)
            .field("relay", &self.relay)
            .finish()
    }
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

/// Options for a local Space (see [`Spaces::create`]).
/// [`Spaces::space`]'s per-Space turn, and the last failure under it.
#[derive(Default)]
struct ConnectGate {
    turn: Arc<tokio::sync::Mutex<()>>,
    failed: std::sync::Mutex<Option<FailedConnect>>,
}

#[derive(Clone)]
struct FailedConnect {
    at: std::time::Instant,
    presence: crate::dial_backoff::Presence,
    reason: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LocalProvision {
    /// Image. Default: the canonical Linux image.
    pub image: Option<String>,
    /// What kind of machine.
    pub kind: Kind,
    /// Which engine.
    pub runtime: Runtime,
    /// Instance name. Default a friendly `adjective-animal`.
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
    /// Whether the image runs cua-spacesd. Default: yes for the Spaces
    /// images (and no image), no for any other image. Only decides how long
    /// to wait for the driver after the sandbox is ready; a Space without
    /// it is still a Space.
    pub spacesd: Option<bool>,
    /// A GPU option ([`SpaceCreate::gpu`]).
    pub gpu: Option<String>,
    /// Host directories ([`SpaceCreate::mounts`]).
    pub mounts: Vec<cua_vmm::Mount>,
    /// No desktop ([`SpaceCreate::headless`]).
    pub headless: bool,
    /// A fork source ([`SpaceCreate::forkable`]).
    pub forkable: bool,
    /// Guest network ([`SpaceCreate::network`]).
    pub network: cua_sandbox_core::NetworkMode,
    /// Egress policy ([`SpaceCreate::egress`]).
    pub egress: crate::egress::EgressPolicy,
    /// Network settings ([`SpaceCreate::network_settings`]).
    pub network_settings: Option<crate::network::NetworkSettings>,
    /// The tree it joins (its root's id); `None`: its own.
    pub network_tree: Option<String>,
}

/// A Space [`Spaces::create`] started with `wait = false`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct PendingSpace {
    /// The id the Space will have.
    pub id: String,
    /// Always `starting` (see `mcp::tools::PHASE_STARTING`).
    pub phase: &'static str,
}

/// How long a connection made without the cua-spacesd its image publishes
/// is kept before the next use dials again.
const SPACESD_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(5);

/// Whether the computer's relay session changed between `then` (when a
/// connection was made) and `now`: online again with another connect time.
/// Unknown either side (no relay, never listed) is no change, and so is an
/// offline listing: the connection may still work over the LAN mesh.
pub(crate) fn session_changed(
    then: crate::dial_backoff::Presence,
    now: crate::dial_backoff::Presence,
) -> bool {
    matches!((then, now), (Some((_, Some(a))), Some((true, Some(b)))) if a != b)
}

/// How long [`Spaces::checked`] waits for a cached connection's cua-spacesd
/// to answer before it dials again. A live one answers in one round trip,
/// even over the relay; a dead one (its cua-spacesd restarted behind the
/// mesh, which sends no reset) would otherwise hold a call until the HTTP/2
/// keepalive gives up (20 s + 10 s).
pub(crate) const LIVE_CHECK: std::time::Duration = std::time::Duration::from_secs(4);

/// The reason a computer the relay lists offline could not be reached,
/// from what failed on the way.
pub(crate) fn listed_offline_reason(failure: &str) -> String {
    use cua_spaces_contract::presence::{LISTED_OFFLINE, says_listed_offline};
    if says_listed_offline(failure) {
        failure.to_string()
    } else {
        format!("{LISTED_OFFLINE} and did not answer ({failure})")
    }
}

/// Whether `space`'s cua-spacesd answers a health check within `within`.
/// Any answer, even a refusal, proves the connection; only a transport
/// failure or silence does not. A Space without cua-spacesd has nothing to
/// ask: it does not.
pub(crate) async fn answers_within(space: &Space, within: Duration) -> bool {
    let Ok(spacesd) = space.spacesd() else {
        return false;
    };
    match tokio::time::timeout(within, spacesd.health()).await {
        Ok(Ok(_)) => true,
        Ok(Err(cua_spacesd_client::Error::Transport(_))) | Err(_) => false,
        Ok(Err(_)) => true,
    }
}

/// How long a cached connection to a computer the relay lists offline has
/// to answer before it is dropped. The relay is the source of presence: a
/// computer listed offline is used only through a connection that answers
/// now (over the LAN mesh it may), never through one cached from before it
/// went away. Over the mesh nothing resets such a connection, so without
/// this check it looked alive for as long as nobody sent a call on it.
pub(crate) const OFFLINE_LIVE_CHECK: Duration = Duration::from_secs(2);

/// Builder for [`Spaces`].
#[derive(Default)]
pub struct SpacesBuilder {
    home: Option<PathBuf>,
    local: Option<Arc<dyn LocalRuntime>>,
    sandboxes: Option<Sandboxes>,
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

    /// Enables Local Spaces on this runtime (the daemon passes its cua-vmm
    /// adapter).
    pub fn local_runtime(mut self, runtime: Arc<dyn LocalRuntime>) -> Self {
        self.local = Some(runtime);
        self
    }

    /// Uses an existing sandbox manager (overrides `local_runtime` for
    /// sandbox lifecycle).
    pub fn sandboxes(mut self, sandboxes: Sandboxes) -> Self {
        self.sandboxes = Some(sandboxes);
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
            if let Some(l) = &self.local {
                b = b.local(l.clone());
            }
            b.build()
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
        let relay_reported = load_reported(&home);
        Spaces {
            inner: Arc::new(Inner {
                #[cfg(feature = "spaces-stream")]
                streams: Default::default(),
                thumbnails,
                #[cfg(feature = "mcp")]
                extensions: self.extensions,
                #[cfg(feature = "spaces-agents")]
                share_consent: std::sync::RwLock::new(self.share_consent),
                registry: Registry::new(home),
                has_local: self.local.is_some(),
                sandboxes,
                operator_display,
                probe_timeout: self.probe_timeout.unwrap_or(DEFAULT_PROBE_TIMEOUT),
                download_dir,
                connections: tokio::sync::Mutex::new(HashMap::new()),
                connected_sessions: Default::default(),
                connecting: Default::default(),
                dial_backoff: Default::default(),
                mesh: crate::mesh::MeshClients::default(),
                #[cfg(feature = "spaces-hotspot")]
                hotspots: tokio::sync::Mutex::new(BTreeMap::new()),
                #[cfg(feature = "spaces-hotspot")]
                exits: Default::default(),
                #[cfg(feature = "spaces-hotspot")]
                host_ports: Default::default(),
                #[cfg(feature = "spaces-hotspot")]
                tree_hubs: Default::default(),
                #[cfg(feature = "spaces-hotspot")]
                overlay: Default::default(),
                #[cfg(feature = "spaces-hotspot")]
                topologies: Default::default(),
                #[cfg(feature = "spaces-agents")]
                relay: std::sync::RwLock::new(self.relay),
                resources: crate::resources::Hub::new(),
                relay_cache: std::sync::Mutex::new(Vec::new()),
                default_cache: std::sync::Mutex::new(None),
                team_facts: std::sync::Mutex::new(Default::default()),
                relay_reported: std::sync::Mutex::new(relay_reported),
                reported_saves: Default::default(),
                relay_read: tokio::sync::Mutex::new(None),
                relay_confirmed: std::sync::atomic::AtomicBool::new(false),
                rounds: Default::default(),
                list_watch: Default::default(),
                site_login: std::sync::RwLock::new(None),
                pool: std::sync::RwLock::new(None),
                durable: std::sync::Mutex::new(Vec::new()),
                elastic: Default::default(),
                pending_volumes: std::sync::Mutex::new(BTreeMap::new()),
            }),
        }
    }
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
        display_name: String::new(),
        provider: Provider::Relay,
        spacesd_version: m.version.clone(),
        features: vec![],
        // What the host said about itself at `cua host setup` (hosts set up
        // before then say nothing: [`Spaces::list`] fills in what its
        // cua-spacesd reported at a connect).
        os: m.meta.get(cua_host::META_OS).cloned().unwrap_or_default(),
        os_name: String::new(),
        os_pretty_name: String::new(),
        image: String::new(),
        image_digest: String::new(),
        kind: String::new(),
        arch: m.meta.get(cua_host::META_ARCH).cloned().unwrap_or_default(),
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
        status: if !m.online {
            "offline".into()
        } else if !m.sharing {
            "not sharing".into()
        } else {
            String::new()
        },
        durable: String::new(),
        stop_reason: String::new(),
        volumes: Vec::new(),
        labels: BTreeMap::new(),
        hidden: false,
        expires_at_unix: 0,
        on_expire: String::new(),
        created_at_unix: 0,
        egress: Default::default(),
        egress_enforced: None,
        network: None,
        tree: None,
        clipboard: Default::default(),
        volume_setting: Default::default(),
        sharing: None,
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

/// What a relay machine's cua-spacesd reported about itself at connect.
/// Kept in `relay-reported.json` in the cua home, so a later process (the
/// CLI, another app) lists the machine's OS without connecting first.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct ReportedFacts {
    os: String,
    os_name: String,
    os_pretty_name: String,
    arch: String,
}

const RELAY_REPORTED_FILE: &str = "relay-reported.json";

#[derive(Default)]
struct ReportedSaves {
    dirty: AtomicBool,
    running: AtomicBool,
}

/// What earlier processes learned from relay machines (empty when none, or
/// unreadable).
fn load_reported(home: &Path) -> HashMap<String, ReportedFacts> {
    std::fs::read(home.join(RELAY_REPORTED_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_reported(home: &Path, reported: &HashMap<String, ReportedFacts>) -> Result<()> {
    let path = home.join(RELAY_REPORTED_FILE);
    cua_home::guard_write(&path)?;
    let sorted: BTreeMap<_, _> = reported.iter().collect();
    cua_home::write_private(&path, &serde_json::to_vec_pretty(&sorted)?)?;
    Ok(())
}

/// The guest OS family from `GetCapabilities`: `linux`, `macos`,
/// `windows`, or empty when not reported.
pub(crate) fn os_family(caps: &cua_proto::env::v1::GetCapabilitiesResponse) -> &'static str {
    use cua_proto::env::v1::OsFamily;
    match caps.os.as_ref().map(|o| o.family()) {
        Some(OsFamily::Linux) => "linux",
        Some(OsFamily::Macos) => "macos",
        Some(OsFamily::Windows) => "windows",
        _ => "",
    }
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

/// A name in the form [`SpaceCreate::name`] defaults to (`adjective-animal`,
/// or the older `space-` and ten lowercase hex digits): one the SDK, or an app on its behalf, generated.
/// A create under such a name cleans up after a failed start, as one with
/// no name does.
pub fn is_generated_name(name: &str) -> bool {
    cua_spaces_contract::names::is_pet_name(name)
        || name.strip_prefix("space-").is_some_and(|hex| {
            hex.len() == 10 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
}

pub(crate) struct Inner {
    /// Windows shown in a Space other than their own ([`crate::stream_registry`]).
    #[cfg(feature = "spaces-stream")]
    pub(crate) streams: crate::stream_registry::StreamRegistry,
    pub(crate) registry: Registry,
    /// Every Space's latest thumbnail ([`crate::thumbnails`]).
    pub(crate) thumbnails: crate::thumbnails::ThumbnailCache,
    /// The registered extensions ([`crate::extension`]).
    #[cfg(feature = "mcp")]
    pub(crate) extensions: Vec<Arc<dyn crate::extension::SpacesExtension>>,
    #[cfg(feature = "spaces-agents")]
    pub(crate) share_consent: std::sync::RwLock<Option<Arc<dyn crate::share::ShareConsent>>>,
    has_local: bool,
    pub(crate) sandboxes: Sandboxes,
    pub(crate) operator_display: Arc<dyn OperatorDisplay>,
    probe_timeout: Duration,
    pub(crate) download_dir: PathBuf,
    connections: tokio::sync::Mutex<HashMap<String, Space>>,
    /// The relay session (online, connected at) of the computer behind each
    /// cached connection when it was made: a computer that reconnected since
    /// (its cua-spacesd restarted) is dialed again ([`session_changed`]).
    connected_sessions: std::sync::Mutex<HashMap<String, crate::dial_backoff::Presence>>,
    /// One connect at a time per Space; callers that arrive meanwhile take
    /// its outcome instead of dialing again.
    connecting: std::sync::Mutex<HashMap<String, Arc<ConnectGate>>>,
    /// Mesh nodes for `relay:` Spaces ([`crate::mesh`]).
    /// Your computers that did not answer, and when to dial them again.
    pub(crate) dial_backoff: crate::dial_backoff::DialBackoff,
    pub(crate) mesh: crate::mesh::MeshClients,
    #[cfg(feature = "spaces-hotspot")]
    pub(crate) hotspots: tokio::sync::Mutex<BTreeMap<String, crate::hotspot::Hotspot>>,
    /// The exits of running Spaces ([`crate::exit`]).
    #[cfg(feature = "spaces-hotspot")]
    pub(crate) exits: crate::exit::ExitTable,
    /// Host ports published in running Spaces ([`crate::host_ports`]).
    #[cfg(feature = "spaces-hotspot")]
    pub(crate) host_ports: crate::host_ports::HostPortsTable,
    /// Containers carried for their tree's VMs ([`crate::tree_hub`]).
    #[cfg(feature = "spaces-hotspot")]
    pub(crate) tree_hubs: crate::tree_hub::TreeHubTable,
    /// The tree overlay across your computers ([`crate::overlay`]).
    #[cfg(feature = "spaces-hotspot")]
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) overlay: crate::overlay::Table,
    /// The topology hint last written into each running Space (a hash).
    #[cfg(feature = "spaces-hotspot")]
    pub(crate) topologies: tokio::sync::Mutex<BTreeMap<String, u64>>,
    #[cfg(feature = "spaces-agents")]
    relay: std::sync::RwLock<Option<crate::relay::RelayAccount>>,
    /// The resource sampler and its watchers ([`crate::resources`]).
    pub(crate) resources: Arc<crate::resources::Hub>,
    /// The last directory listing (so `list` and `resolve` stay sync).
    pub(crate) relay_cache: std::sync::Mutex<Vec<crate::relay::RelayMachine>>,
    /// The account's default Space as the relay last said (or none), and
    /// when it was read: lists mark it without asking every time.
    pub(crate) default_cache: std::sync::Mutex<Option<(std::time::Instant, Option<String>)>>,
    /// Team names and roles from the Spaces API ([`crate::sharing`]).
    pub(crate) team_facts: std::sync::Mutex<crate::sharing::TeamFacts>,
    /// What each relay machine's cua-spacesd said about itself when this
    /// process last connected to it (by `relay:<id>`): the directory
    /// listing carries no OS, so without it a Mac lists as Linux.
    relay_reported: std::sync::Mutex<HashMap<String, ReportedFacts>>,
    /// Whether `relay_reported` has changes not yet on disk, and whether a
    /// save is running ([`Spaces::remember_reported`]).
    reported_saves: ReportedSaves,
    /// Held across a read so only one directory read runs at a time.
    relay_read: tokio::sync::Mutex<Option<std::time::Instant>>,
    /// The relay answered a directory read in this process (until then the
    /// directory is the one saved by an earlier process, if any).
    pub(crate) relay_confirmed: std::sync::atomic::AtomicBool,
    /// The read of the account's computers [`Spaces::list_known`] runs.
    pub(crate) rounds: crate::listing::AccountRounds,
    /// The list's watcher, while anything subscribes
    /// ([`Spaces::watch_list`]).
    pub(crate) list_watch: crate::list_watch::ListWatchSlot,
    /// The Keyvault broker `request_site_login` signs in through (the
    /// daemon sets it once its Keyvault is up).
    site_login: std::sync::RwLock<Option<Arc<dyn crate::site_login::SiteLoginBroker>>>,
    /// What reaches the coordinator of the account's machines (durable
    /// Spaces; the cua daemon of Cua Spaces sets it).
    pub(crate) pool: std::sync::RwLock<Option<Arc<dyn crate::durable::PoolRequests>>>,
    /// Durable Spaces as last known: (name, the Space it is now).
    pub(crate) durable: std::sync::Mutex<Vec<(String, String)>>,
    /// The elastic pool's placements and demand ([`crate::elastic`]).
    pub(crate) elastic: crate::elastic::ElasticState,
    /// Named volumes of Spaces being created, until their record exists.
    pub(crate) pending_volumes:
        std::sync::Mutex<BTreeMap<String, Vec<crate::named_volumes::VolumeMount>>>,
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

    /// Frees capacity this machine holds for nobody (idle warm-pool members,
    /// VMs whose lifetime passed or whose owner is gone) so a create that
    /// was short of CPU or memory can look again.
    pub async fn reclaim(&self, need: cua_sandbox_core::Shortfall) -> cua_sandbox_core::Reclaimed {
        self.inner.sandboxes.reclaim(need).await
    }

    /// The sandbox manager Local Spaces are provisioned with.
    pub fn sandboxes(&self) -> &Sandboxes {
        &self.inner.sandboxes
    }

    /// The operator display.
    pub fn operator_display(&self) -> &Arc<dyn OperatorDisplay> {
        &self.inner.operator_display
    }

    // ------------------------------------------------------------ registry

    /// A friendly name (`brave-otter`) that no registered Space has
    /// ([`cua_spaces_contract::names::pet_name`]): the default of a create without one.
    pub fn fresh_name(&self) -> String {
        let taken: std::collections::HashSet<String> = self
            .inner
            .registry
            .list()
            .map(|v| v.into_iter().map(|s| s.name).collect())
            .unwrap_or_default();
        cua_spaces_contract::names::pet_name(|n| taken.contains(n))
    }

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
                info.stop_reason = self.stop_reason_of(&info.id);
                Ok(info)
            })
            .collect::<Result<_>>()?;
        // A Space this machine provides is in the registry as a local Space,
        // and its relay machine lists it again under the host: once, as the
        // local one (CUA-1527).
        let provided: Vec<cua_host::provided::ProvidedSpace> =
            cua_host::provided::load_provided(&cua_host::Host::new(self.home_dir()).paths().dir)
                .unwrap_or_default()
                .into_iter()
                .filter(|p| out.iter().any(|i| i.id == p.local_space))
                .collect();
        let facts = self.team_facts();
        let provided_here: std::collections::HashSet<String> =
            provided.iter().map(|p| p.relay_machine.clone()).collect();
        // A Space of this computer that is on the account (listed by Cua
        // itself, by hand, or kept there because it is shared with other
        // people) is the local one, once: every attached machine, not only
        // the ones a pass listed.
        #[cfg(feature = "spaces-agents")]
        let provided_here = {
            let mut here = provided_here;
            here.extend(self.attached_machines().into_values());
            here
        };
        let cache = self.inner.relay_cache.lock().expect("relay cache");
        // Its name on the relay is the one your other devices see: a rename
        // there (from another device) shows here too. The name it was
        // registered with (or its machine id, when it had none) is no
        // rename.
        let relay_names: Vec<(String, String)> = provided
            .iter()
            .filter_map(|p| {
                let m = cache.iter().find(|m| m.id == p.relay_machine)?;
                (!m.name.is_empty() && m.name != p.name && m.name != p.relay_machine)
                    .then(|| (p.local_space.clone(), m.name.clone()))
            })
            .collect();
        for m in cache.iter() {
            if provided_here.contains(&m.id) {
                continue;
            }
            let mut info = relay_info(m);
            info.sharing = Some(crate::sharing::sharing_of(m, &facts));
            self.apply_reported(&mut info, m.online);
            self.apply_volumes(&mut info);
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
        drop(cache);
        #[cfg(feature = "spaces-agents")]
        self.apply_attached_sharing(&mut out);
        self.mark_durable(&mut out);
        self.apply_meta(&mut out);
        for (local, relay_name) in relay_names {
            if let Some(info) = out.iter_mut().find(|i| i.id == local)
                && relay_name != info.name
            {
                info.display_name = relay_name;
            }
        }
        Ok(out)
    }

    /// Refreshes the relay directory when an account is configured, then
    /// lists every Space. A failed refresh is an error, not an empty or
    /// current directory (a brief relay hiccup lists the last directory
    /// for a while, see `relay_directory`), except when no account is
    /// signed in at all: that
    /// is the normal state of many hosts, so this machine's own Spaces are
    /// listed without the account's machines (and any listed before the
    /// sign-out are dropped). [`Self::list`] remains the cached, local read.
    pub async fn list_all(&self) -> Result<Vec<SpaceInfo>> {
        if let Err(e) = self.durable_spaces().await {
            tracing::debug!(error = %e, "durable Spaces not listed");
        }
        if let Some(account) = self.relay_account()
            && let Err(e) = self.relay_machines().await
        {
            if e.tag() != "unauthenticated" || account.tokens.signed_in().await {
                return Err(e);
            }
            tracing::debug!("relay directory skipped: not signed in");
            self.forget_account_computers();
        }
        self.refresh_team_facts().await;
        self.list()
    }

    // --------------------------------------------------------------- relay

    /// This install's relay machine id (`cua host setup` in relay mode),
    /// when it has one: the relay listing's entry for this machine itself
    /// (apps show it in its own place, not among "your machines").
    pub fn this_relay_machine_id(&self) -> Option<String> {
        cua_host::Host::new(self.home_dir())
            .config()
            .ok()
            .flatten()
            .and_then(|c| c.machine_id)
            .filter(|id| !id.is_empty())
    }

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
    /// when `cua daemon` picks up the signed-in session. The cached
    /// directory listing becomes the one an earlier process saved for this
    /// relay (`relay-directory.json`), unconfirmed until the relay answers.
    pub fn set_relay(&self, relay: Option<crate::relay::RelayAccount>) {
        let saved = relay
            .as_ref()
            .map(|r| crate::listing::load_directory(self.home_dir(), &r.url))
            .unwrap_or_default();
        *self.inner.relay.write().expect("relay") = relay;
        *self.inner.relay_cache.lock().expect("relay cache") = saved;
        self.inner
            .relay_confirmed
            .store(false, std::sync::atomic::Ordering::Release);
        self.inner.rounds.reset();
    }

    /// `e`, from a call on `space`, with the reason when the call failed
    /// because the Space's mesh connection was cut (a revoked device, the
    /// mesh turned off): a transport error alone ("broken pipe") does not
    /// say why. Other errors are returned as they are.
    pub async fn explain_mesh_cut(&self, space: &str, e: Error) -> Error {
        if !matches!(e, Error::Env(cua_spacesd_client::Error::Transport(_))) {
            return e;
        }
        let Ok(id) = self.resolve(space) else {
            return e;
        };
        match self
            .inner
            .mesh
            .cut_reason(&id.to_string(), Duration::from_secs(2))
            .await
        {
            Some(reason) => Error::host(
                "the mesh",
                format!("the connection to {id} over the mesh was cut: {reason}"),
            ),
            None => e,
        }
    }

    /// The mesh node of each relay this runtime reached over the mesh:
    /// its peers and, per peer, the path (direct or relayed) and counters.
    pub async fn mesh_status(&self) -> Vec<(String, cua_mesh::Status)> {
        self.inner.mesh.status().await
    }

    /// Drops the mesh's loopback forwards and the connections they carry,
    /// to give descriptors back when the process runs short (the daemon's
    /// descriptor guard). Spaces over the mesh reconnect on their next
    /// call. Returns how many forwards were dropped.
    pub async fn shed_mesh_forwards(&self) -> usize {
        let (n, spaces) = self.inner.mesh.shed_forwards().await;
        let mut connections = self.inner.connections.lock().await;
        for space in spaces {
            connections.remove(&space);
        }
        n
    }

    pub(crate) fn relay(&self) -> Result<crate::relay::RelayAccount> {
        self.relay_account().ok_or_else(|| {
            Error::host(
                "cua.ai relay account",
                "sign in to cua.ai (`cua auth login` or the app) to list your computers",
            )
        })
    }

    /// The machines of the signed-in account on the relay (owned and shared
    /// with it), each reachable as `relay:<id>`.
    pub async fn relay_machines(&self) -> Result<Vec<crate::relay::RelayMachine>> {
        self.relay_directory(None).await
    }

    const RELAY_MAX_AGE: Duration = Duration::from_secs(10);

    pub(crate) async fn relay_directory(
        &self,
        want: Option<&str>,
    ) -> Result<Vec<crate::relay::RelayMachine>> {
        let relay = self.relay()?;
        let mut read_at = self.inner.relay_read.lock().await;
        let cached = self.inner.relay_cache.lock().expect("relay cache").clone();
        let listed = want.is_some_and(|id| cached.iter().any(|m| m.id == id));
        if listed && read_at.is_some_and(|at| at.elapsed() < Self::RELAY_MAX_AGE) {
            return Ok(cached);
        }
        let read = async {
            let token = relay.tokens.access_token().await?;
            let client = cua_host::RelayClient::new(&relay.url)?;
            Ok::<_, Error>(match &relay.device {
                Some(d) => d.machines_for(&client, &token).await?,
                None => client.machines(&token).await?,
            })
        };
        let machines = match read.await {
            Ok(m) => m,
            Err(_) if listed => return Ok(cached),
            // The relay briefly unreachable (a deploy, a moment with no
            // backend behind its proxy): the machines it listed a moment
            // ago stand, each with its own state, instead of failing the
            // whole Space list. Past the grace the failure shows.
            Err(Error::Relay(e))
                if e.is_transient()
                    && !cached.is_empty()
                    && read_at.is_some_and(|at| at.elapsed() < cua_host::RELAY_HICCUP_GRACE) =>
            {
                tracing::debug!("relay directory read failed; listing its last answer: {e}");
                return Ok(cached);
            }
            Err(e) => {
                return Err(Error::Relay(match e.tag() {
                    "unauthenticated" => cua_host::Error::Unauthenticated(
                        "Relay authentication failed. Sign in again to refresh your computers."
                            .into(),
                    ),
                    "permission_denied" => cua_host::Error::PermissionDenied(
                        "This device could not list relay computers. Check its account access."
                            .into(),
                    ),
                    _ => cua_host::Error::Relay("Could not refresh relay computers.".into()),
                }));
            }
        };
        *read_at = Some(std::time::Instant::now());
        let changed = {
            let mut cache = self.inner.relay_cache.lock().expect("relay cache");
            let changed = *cache != machines;
            *cache = machines.clone();
            changed
        };
        let first = !self
            .inner
            .relay_confirmed
            .swap(true, std::sync::atomic::Ordering::AcqRel);
        // Kept for the next process: it lists these at once, before the
        // relay answers it (`Spaces::list_known`).
        if changed || first {
            crate::listing::save_directory(self.home_dir(), &relay.url, &machines);
        }
        Ok(machines)
    }

    /// Whether relay host `machine` routes Spaces of this device (they are
    /// reached through its channel: [`crate::id::routed_authority`]).
    fn routes_through(&self, machine: &str) -> bool {
        let space = format!("relay:{machine}");
        self.inner
            .registry
            .direct_hosts()
            .is_ok_and(|hosts| hosts.iter().any(|h| h.space == space))
    }

    /// Whether relay machine `machine_id` is a Space one of your own hosts
    /// provides (`on="host:<machine>"`, `on="auto"`): its directory row or
    /// registry record (`record`) names its host. Reads the directory once
    /// when this process has not listed it yet.
    pub(crate) async fn provided_by_a_host(
        &self,
        machine_id: &str,
        record: Option<&dpb::Space>,
    ) -> bool {
        let known = |s: &Self| {
            let cached = s
                .inner
                .relay_cache
                .lock()
                .expect("relay cache")
                .iter()
                .find(|m| m.id == machine_id)
                .map(|m| m.host.as_deref().is_some_and(|h| !h.is_empty()));
            let recorded = record.map(|r| !r.host.is_empty());
            match (cached, recorded) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (None, None) => None,
                _ => Some(false),
            }
        };
        if let Some(known) = known(self) {
            return known;
        }
        if let Err(e) = self.relay_directory(Some(machine_id)).await {
            tracing::debug!(machine = %machine_id, error = %e, "relay directory not read");
        }
        known(self).unwrap_or(false)
    }

    /// The listed row of `space`. Only a `relay:` id reads the directory.
    pub async fn find(&self, space: &str) -> Result<SpaceInfo> {
        let id = self.resolve(space)?;
        let key = id.to_string();
        if let SpaceId::Relay { machine_id } = &id
            && self.relay_account().is_some()
            && self.inner.registry.get(&key)?.is_none()
        {
            self.relay_directory(Some(machine_id)).await?;
        }
        self.list()?
            .into_iter()
            .find(|i| i.id == key)
            .ok_or_else(|| Error::NotFound(format!("Space {key}")))
    }

    pub(crate) fn relay_forget(&self, machine_id: &str) {
        self.inner
            .relay_cache
            .lock()
            .expect("relay cache")
            .retain(|m| m.id != machine_id);
    }

    /// Resolves any accepted spelling (canonical id, legacy id, URL, display
    /// name, short name) to a canonical id.
    pub fn resolve(&self, space: &str) -> Result<SpaceId> {
        if let Ok(id) = SpaceId::parse(space) {
            return Ok(id);
        }
        // A durable Space's name is the Space it is now, wherever it is.
        if let Some((_, now)) = self.durable_of(space)
            && let Ok(id) = SpaceId::parse(&now)
        {
            return Ok(id);
        }
        let records = self.inner.registry.list()?;
        let mut matches: Vec<SpaceId> = records
            .iter()
            .filter_map(|r| SpaceId::parse(&r.id).ok().map(|id| (r, id)))
            .filter(|(r, id)| r.name == space || id.short_name() == space)
            .map(|(_, id)| id)
            .collect();
        // The name it was renamed to.
        for (key, m) in self.inner.registry.all_meta() {
            if m.display_name == space
                && let Ok(id) = SpaceId::parse(&key)
                && !matches.contains(&id)
            {
                matches.push(id);
            }
        }
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

    /// Forks `source` (a local sandbox, running or stopped, or a
    /// checkpoint) as the local Space `name` and registers it: a QEMU fork
    /// of a memory checkpoint maps its RAM copy-on-write, so many forks of
    /// one checkpoint share every page they do not write.
    pub async fn fork(&self, source: &str, name: &str) -> Result<SpaceInfo> {
        self.fork_local(source, Some(name.to_string())).await
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
            || ["local:", "cloud:", "direct:", "relay:"]
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
        let record = self.inner.registry.get(&id.to_string())?;
        let space = match self
            .connect(&id, &credential, name.as_deref(), record.as_ref())
            .await
        {
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
                let space = self
                    .connect(&id, &credential, name.as_deref(), record.as_ref())
                    .await?;
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
        #[cfg(feature = "spaces-stream")]
        self.drop_stream_entries(space);
        let id = self.resolve(space)?;
        let info = self.info(&id).ok();
        let _ = crate::lifetimes::forget(self.home_dir(), &id.to_string());
        self.drop_connection(&id).await;
        // A Space that registered itself on the relay and is gone (its
        // machine is not connected) leaves the relay directory: otherwise
        // its stale record would list again on the next refresh. Only its
        // owner can remove it; anything else is only dropped here.
        if let SpaceId::Relay { machine_id } = &id
            && self.relay_account().is_some()
            && let Some(m) = self
                .relay_machines()
                .await
                .ok()
                .and_then(|all| all.into_iter().find(|m| &m.id == machine_id))
            && m.role == "owner"
            && !m.online
            && crate::relay::is_registered_space(&m)
        {
            self.forget_relay_machine(machine_id, false).await?;
        }
        // A Space this device attached to the relay as a machine of its own
        // takes that machine with it (as `delete` does): `rm` must not leave
        // a stale record the owner has to remove by hand (CUA-2841).
        #[cfg(feature = "spaces-agents")]
        if !matches!(id, SpaceId::Relay { .. }) {
            let _ = self.release_attached(&id.to_string()).await;
        }
        // A direct host leaves the direct-host list with the Spaces it
        // created here (nothing reaches them without it; a host that is
        // gone leaves no rows behind), and a Space one created leaves its
        // host's list.
        if matches!(id, SpaceId::Direct { .. }) {
            let is_host = self
                .inner
                .registry
                .direct_hosts()
                .is_ok_and(|hosts| hosts.iter().any(|h| h.space == id.to_string()));
            if is_host {
                self.forget_host_spaces(&id.to_string()).await;
            }
            let _ = self.inner.registry.forget_direct(&id.to_string());
        }
        if !self.inner.registry.remove(&id.to_string())? && !matches!(id, SpaceId::Relay { .. }) {
            return Err(Error::NotFound(id.to_string()));
        }
        self.inner.thumbnails.remove(&id.to_string());
        Ok(info.unwrap_or_else(|| SpaceInfo {
            id: id.to_string(),
            name: id.short_name().into(),
            display_name: String::new(),
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
            status: String::new(),
            durable: String::new(),
            stop_reason: String::new(),
            volumes: Vec::new(),
            labels: BTreeMap::new(),
            hidden: false,
            expires_at_unix: 0,
            on_expire: String::new(),
            created_at_unix: 0,
            egress: Default::default(),
            egress_enforced: None,
            network: None,
            tree: None,
            clipboard: Default::default(),
            volume_setting: Default::default(),
            sharing: None,
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
        let _ = self
            .inner
            .registry
            .set_overlay_volumes(&id.to_string(), vec![]);
    }

    pub(crate) fn info(&self, id: &SpaceId) -> Result<SpaceInfo> {
        let mut info = [self.info_unlabelled(id)?];
        self.apply_meta(&mut info);
        let [info] = info;
        Ok(info)
    }

    /// Fills in what is kept apart from the handshake record: labels,
    /// when the Space was created, and its lifetime.
    fn apply_meta(&self, infos: &mut [SpaceInfo]) {
        let meta = self.inner.registry.all_meta();
        let lifetimes = crate::lifetimes::all(self.home_dir());
        for info in infos {
            if let Some(m) = meta.get(&info.id) {
                if !m.display_name.is_empty() {
                    info.display_name = m.display_name.clone();
                }
                info.labels = m.labels.clone();
                info.hidden = m.labels.contains_key(crate::registry::HIDDEN_LABEL);
                if m.created_at_unix > 0 {
                    info.created_at_unix = m.created_at_unix;
                }
                info.egress = m.egress.clone();
                info.clipboard = m.clipboard;
                info.volume_setting = m.volume;
                info.egress_enforced =
                    (!m.egress.is_empty()).then_some(crate::egress::Enforcement::Host);
                info.network = Some(crate::network::SpaceNetwork {
                    settings: crate::network::settings_of(Some(m)),
                    note: m.network_note.clone(),
                    notice_pending: m.network_notice_pending,
                });
                info.tree = Some(crate::tree::TreeLink::of(m));
            }
            if let Some(l) = self.lifetime_of(&info.id, &lifetimes) {
                info.expires_at_unix = l.expires_at_unix;
                info.on_expire = l.on_expire.clone();
            }
        }
    }

    /// Gives Space `new` the labels and egress policy of Space `source`:
    /// what a fork of a Space starts with. The runtime's fork carries the
    /// policy itself (`cua_vmm::Runtime::fork`), so it is enforced from
    /// the fork's first instruction; a fork made any other way must call
    /// [`Spaces::set_egress`]. Its creation time and lifetime are its own.
    /// Returns whether `source` had any.
    pub fn inherit_meta(&self, source: &str, new: &str) -> Result<bool> {
        let (source, new) = (self.resolve(source)?, self.resolve(new)?);
        self.inner
            .registry
            .inherit_meta(&source.to_string(), &new.to_string())
    }

    /// [`Spaces::apply_meta`] for registry records (what the daemon serves):
    /// labels, creation time and lifetime filled in.
    pub fn fill_records(&self, records: &mut [cua_proto::daemon::v1::Space]) {
        let meta = self.inner.registry.all_meta();
        let lifetimes = crate::lifetimes::all(self.home_dir());
        let tree_addrs = cua_vmm::tree::addresses();
        for r in records {
            r.created_at_unix = r.added_at.as_ref().map_or(0, |t| t.seconds.max(0));
            if let Some(m) = meta.get(&r.id) {
                if !m.display_name.is_empty() {
                    r.display_name = m.display_name.clone();
                }
                r.labels = m
                    .labels
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                if m.created_at_unix > 0 {
                    r.created_at_unix = i64::try_from(m.created_at_unix).unwrap_or(0);
                }
                r.egress = (!m.egress.is_empty()).then(|| cua_proto::daemon::v1::EgressPolicy {
                    allow: m.egress.allow.clone(),
                    deny: m.egress.deny.clone(),
                });
                r.clipboard = m.clipboard.as_str().into();
                r.volume = m.volume.as_str().into();
                r.egress_enforced = if m.egress.is_empty() {
                    String::new()
                } else {
                    crate::egress::Enforcement::Host.as_str().into()
                };
                r.network_settings = m.network.as_ref().map(crate::network::to_pb);
                r.network_note = m.network_note.clone().unwrap_or_default();
                r.network_notice_pending = m.network_notice_pending;
                r.parent = m.parent.clone().unwrap_or_default();
                r.root = m.root.clone().unwrap_or_default();
                r.depth = m.depth;
                r.origin = crate::tree::TreeLink::of(m).origin;
            }
            // A VM's address in its tree ([`cua_vmm::tree`]).
            if let Some(a) = r.id.strip_prefix("local:").and_then(|n| tree_addrs.get(n)) {
                r.tree_address = a.to_string();
            }
            if let Some(l) = self.lifetime_of(&r.id, &lifetimes) {
                r.expires_at_unix = i64::try_from(l.expires_at_unix).unwrap_or(0);
                r.on_expire = l.on_expire.clone();
            }
        }
    }

    fn info_unlabelled(&self, id: &SpaceId) -> Result<SpaceInfo> {
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
            let mut info = relay_info(m);
            info.sharing = Some(crate::sharing::sharing_of(m, &self.team_facts()));
            self.apply_reported(&mut info, m.online);
            self.apply_volumes(&mut info);
            return Ok(info);
        }
        let record = self
            .inner
            .registry
            .get(&id.to_string())?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;
        SpaceInfo::from_record(&record)
    }

    /// Keeps what a relay machine's cua-spacesd answered at connect (its
    /// OS and architecture), for its listing, here and on disk.
    fn remember_reported(&self, key: &str, space: &Space) {
        let (os_name, os_pretty_name) = os_names(space);
        let (_, arch) = space.platform();
        let facts = ReportedFacts {
            os: os_family(space.capabilities()).into(),
            os_name,
            os_pretty_name,
            arch,
        };
        {
            let mut reported = self.inner.relay_reported.lock().expect("relay reported");
            if reported.get(key) == Some(&facts) {
                return;
            }
            reported.insert(key.to_string(), facts);
        }
        // Saved off the connect path, one save at a time: a launch connects
        // to every Space at once, and a synced write per connect held up
        // the runtime. A change made while a save runs gets one more save.
        let saves = &self.inner.reported_saves;
        saves.dirty.store(true, Ordering::SeqCst);
        if saves.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let saves = &inner.reported_saves;
            loop {
                while saves.dirty.swap(false, Ordering::SeqCst) {
                    let snapshot = inner.relay_reported.lock().expect("relay reported").clone();
                    // Best effort: the listing in this process has it either way.
                    if let Err(e) = save_reported(inner.registry.dir(), &snapshot) {
                        tracing::debug!(error = %e, "relay-reported.json not written");
                    }
                }
                saves.running.store(false, Ordering::SeqCst);
                if !saves.dirty.load(Ordering::SeqCst) || saves.running.swap(true, Ordering::SeqCst)
                {
                    return;
                }
            }
        });
    }

    /// A relay machine's listing, with what its cua-spacesd reported (it
    /// knows best: the distribution, the version) over what the relay
    /// directory says. Only while the machine is online: an offline host's listing never
    /// carries what an earlier connect learned (it may be stale by its next
    /// connect), the same rule a discovered machine's connected info follows.
    fn apply_reported(&self, info: &mut SpaceInfo, online: bool) {
        if !online {
            return;
        }
        let reported = self.inner.relay_reported.lock().expect("relay reported");
        let Some(f) = reported.get(&info.id) else {
            return;
        };
        let fill = |field: &mut String, value: &str| {
            if !value.is_empty() {
                *field = value.to_string();
            }
        };
        fill(&mut info.os, &f.os);
        fill(&mut info.os_name, &f.os_name);
        fill(&mut info.os_pretty_name, &f.os_pretty_name);
        fill(&mut info.arch, &f.arch);
    }

    /// Brings a registered Space's handshake facts (OS, image) up to date
    /// after a connect, when they changed (a newer driver, a record written
    /// before these fields existed). Best effort: the connect stands.
    fn refresh_record(
        &self,
        key: &str,
        record: &dpb::Space,
        space: &Space,
        credential: &Credential,
    ) {
        let (image, digest) = space.image().unwrap_or_default();
        let (os_name, os_pretty_name) = os_names(space);
        let (kind, arch) = space.platform();
        let stale = record.os_pretty_name != os_pretty_name
            || record.os_name != os_name
            || record.kind != kind
            || record.arch != arch
            || (!image.is_empty() && (record.image != image || record.image_digest != digest));
        // Still registered: a Space removed while this connect ran stays
        // removed.
        if stale
            && space.has_spacesd()
            && matches!(self.inner.registry.get(key), Ok(Some(_)))
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
            os: os_family(caps).into(),
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
            image_config: existing
                .as_ref()
                .map(|r| r.image_config.clone())
                .unwrap_or_default(),
            volumes: match existing.as_ref().map(|r| r.volumes.clone()) {
                Some(v) if !v.is_empty() => v,
                _ => self.take_expected_volumes(&space.id().to_string()),
            },
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
        let id = self.resolve_now(space).await?;
        let key = id.to_string();
        if let Some(s) = self.cached_connection(&key).await {
            return Ok(s);
        }
        // One connect per Space at a time, run on its own: an app polling
        // the list gives up on a computer that does not answer and asks
        // again before the attempt ended. The attempt goes on (a caller's
        // deadline does not cancel it), and its outcome answers whoever
        // asked meanwhile, so a slow computer is dialed once, not once a
        // poll.
        let gate = self
            .inner
            .connecting
            .lock()
            .expect("connecting")
            .entry(key.clone())
            .or_default()
            .clone();
        let arrived = std::time::Instant::now();
        let me = self.clone();
        let attempt = tokio::spawn(async move {
            let _turn = gate.turn.clone().lock_owned().await;
            if let Some(s) = me.inner.connections.lock().await.get(&key).cloned() {
                return Ok(s);
            }
            // What the relay says about the computer now: one that came
            // back (online again, or a new session) is dialed afresh.
            let presence = match &id {
                SpaceId::Relay { machine_id } => me.relay_presence(machine_id),
                _ => None,
            };
            if let Some(f) = gate.failed.lock().expect("gate").clone()
                && f.at >= arrived
                && f.presence == presence
            {
                return Err(Error::SpacesdNotAvailable {
                    space: key,
                    reason: f.reason,
                });
            }
            let r = me.connect_space(id, key).await;
            if let Err(e) = &r {
                let reason = match e {
                    Error::SpacesdNotAvailable { reason, .. } => reason.clone(),
                    e => e.to_string(),
                };
                *gate.failed.lock().expect("gate") = Some(FailedConnect {
                    at: std::time::Instant::now(),
                    presence,
                    reason,
                });
            }
            r
        });
        attempt
            .await
            .map_err(|e| Error::Cancelled(format!("the connect to this Space ended: {e}")))?
    }

    /// Whether the registry remembers cua-spacesd answering for `id`: its
    /// image runs one, so a connection without it is a Space still waking.
    pub(crate) fn record_expects_spacesd(&self, id: &SpaceId) -> bool {
        record_runs_spacesd(
            self.inner
                .registry
                .get(&id.to_string())
                .ok()
                .flatten()
                .as_ref(),
        )
    }

    /// The open connection to `key`, unless its route went stale or it can
    /// move to the mesh now (then it is dropped, to connect again).
    async fn cached_connection(&self, key: &str) -> Option<Space> {
        let key = key.to_string();
        // Bound first: the cache's guard must be gone before the lock is
        // taken again below.
        let cached = self.inner.connections.lock().await.get(&key).cloned();
        // Connected while its cua-spacesd didn't answer (the Space was still
        // waking, as after a daemon restart): dial again, or the Space stays
        // without spacesd until it is restarted and agents can't start there.
        if let Some(s) = &cached
            && s.spacesd_retry_due(SPACESD_RETRY_AFTER)
        {
            tracing::info!(space = %key, "no cua-spacesd answered when this connected; reconnecting");
            self.inner.connections.lock().await.remove(&key);
            return None;
        }
        if cached.is_some()
            && let Some(reason) = self.peer_reconnected(&key)
        {
            tracing::info!(space = %key, %reason, "its computer reconnected; reconnecting");
            self.forget_stale_connection(&key).await;
            return None;
        }
        // Its computer left the relay (its app quit, it slept, it lost the
        // network): the connection stands only if it still answers.
        if let Some(s) = &cached
            && self.carrier_listed_offline(&key)
            && !answers_within(s, OFFLINE_LIVE_CHECK).await
        {
            tracing::info!(space = %key, "its computer is offline and the connection does not answer; dropping it");
            self.forget_stale_connection(&key).await;
            return None;
        }
        if let Some(s) = cached {
            let policy = |provided| {
                if provided {
                    crate::mesh::policy_for_provided()
                } else {
                    crate::mesh::policy()
                }
            };
            match self.inner.mesh.stale(&key, policy).await {
                None => match self
                    .inner
                    .mesh
                    .upgrade(&key, crate::mesh::UPGRADE_CHECK)
                    .await
                {
                    None => return Some(s),
                    Some(reason) => {
                        tracing::info!(space = %key, %reason, "moving to the mesh");
                        self.inner.connections.lock().await.remove(&key);
                    }
                },
                Some(reason) => {
                    tracing::info!(space = %key, %reason, "the mesh went away; reconnecting");
                    self.inner.connections.lock().await.remove(&key);
                }
            }
        }
        None
    }

    /// [`Spaces::space`] behind a type-erased future: the routed Space's
    /// connect awaits its host's, and the erased type ends the recursion.
    fn space_of_host(
        &self,
        host: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Space>> + Send + '_>> {
        Box::pin(async move { self.space(&host).await })
    }

    async fn connect_space(&self, id: SpaceId, key: String) -> Result<Space> {
        // Read once: everything below this connect asks this record.
        let record = self.inner.registry.get(&key)?;
        let name = match (&id, &record) {
            (_, Some(record)) => Some(record.name.clone()),
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
        // One of your computers that the relay lists as offline, and that
        // did not answer, is not dialed again on every poll
        // (`crate::dial_backoff`). An online one is always dialed: its
        // cua-spacesd may be a moment from connecting (`cua spaces start`).
        let presence = match &id {
            SpaceId::Relay { machine_id } => {
                Some(self.relay_presence(machine_id)).filter(|p| matches!(p, Some((false, _))))
            }
            _ => None,
        };
        if let Some(presence) = presence
            && let Some(reason) =
                self.inner
                    .dial_backoff
                    .check(&key, presence, std::time::Instant::now())
        {
            return Err(Error::SpacesdNotAvailable { space: key, reason });
        }
        let s = match self
            .connect(&id, &credential, name.as_deref(), record.as_ref())
            .await
        {
            Ok(s) => {
                self.inner.dial_backoff.clear(&key);
                s
            }
            Err(e) => {
                if let Some(presence) = presence
                    && !matches!(e, Error::NotFound(_) | Error::InvalidArgument(_))
                {
                    // Offline, and it did not answer either: it is offline,
                    // whatever went wrong on the way (the apps read the
                    // reason as offline: `presence::LISTED_OFFLINE`).
                    let reason = listed_offline_reason(&e.to_string());
                    self.inner.dial_backoff.fail(
                        &key,
                        presence,
                        reason.clone(),
                        std::time::Instant::now(),
                    );
                    return Err(Error::SpacesdNotAvailable { space: key, reason });
                }
                return Err(e);
            }
        };
        match &record {
            Some(record) => self.refresh_record(&key, record, &s, &credential),
            None if matches!(id, SpaceId::Relay { .. }) => self.remember_reported(&key, &s),
            None => {}
        }
        let session = self.session_of(&key);
        self.inner
            .connected_sessions
            .lock()
            .expect("sessions")
            .insert(key.clone(), session);
        self.inner.connections.lock().await.insert(key, s.clone());
        #[cfg(feature = "mcp")]
        self.notify_connected(&id);
        Ok(s)
    }

    /// What the relay's directory last said about `machine_id`: online,
    /// and when it last connected ([`crate::dial_backoff::Presence`]).
    fn relay_presence(&self, machine_id: &str) -> crate::dial_backoff::Presence {
        self.inner
            .relay_cache
            .lock()
            .expect("relay cache")
            .iter()
            .find(|m| m.id == machine_id)
            .map(|m| (m.online, m.connected_at))
    }

    /// The relay session of the computer that carries Space `key`: the
    /// relay machine itself, or the host a routed Space rides on. `None`
    /// for a Space no relay computer carries.
    fn session_of(&self, key: &str) -> crate::dial_backoff::Presence {
        match SpaceId::parse(key).ok()? {
            SpaceId::Relay { machine_id } => self.relay_presence(&machine_id),
            SpaceId::Direct { authority } => {
                let (machine, _) = crate::id::routed_host(&authority)?;
                self.relay_presence(machine)
            }
            _ => None,
        }
    }

    /// Why the cached connection to `key` is to a cua-spacesd that is gone:
    /// the relay says its computer connected again since (Set up sharing
    /// This Mac's desktop, an update or a restart starts its cua-spacesd
    /// again). Over the mesh nothing resets the old connection, so without
    /// this its next call waits out the keepalive and fails.
    fn peer_reconnected(&self, key: &str) -> Option<String> {
        let then = *self
            .inner
            .connected_sessions
            .lock()
            .expect("sessions")
            .get(key)?;
        session_changed(then, self.session_of(key))
            .then(|| "the relay says it connected again since".to_string())
    }

    /// Forgets the cached connection to `key` (its next use dials again),
    /// keeping what [`crate::dial_backoff`] learned: unlike
    /// [`Spaces::forget_connection`], nobody asked for a dial. Unlike
    /// `drop_connection`, the Space's hotspot and what extensions attached
    /// (a mounted volume) stay: the Space is the same, only its connection
    /// is new.
    pub(crate) async fn forget_stale_connection(&self, key: &str) {
        self.inner.connections.lock().await.remove(key);
        self.inner
            .connected_sessions
            .lock()
            .expect("sessions")
            .remove(key);
    }

    /// `space`, or a new connection to it when its cua-spacesd does not
    /// answer a health check within [`LIVE_CHECK`]. For a call that must
    /// not hang on a dead connection and is not safe to send twice (starting
    /// an agent): the check has no side effect, so the call itself goes
    /// out once, on a connection that just answered. Any answer, even a
    /// refusal, proves the connection; only a transport failure or silence
    /// dials again.
    pub async fn checked(&self, space: Space) -> Result<Space> {
        let Ok(spacesd) = space.spacesd() else {
            return Ok(space);
        };
        let reason = match tokio::time::timeout(LIVE_CHECK, spacesd.health()).await {
            Ok(Ok(_)) => return Ok(space),
            Ok(Err(cua_spacesd_client::Error::Transport(e))) => e,
            Ok(Err(_)) => return Ok(space),
            Err(_) => format!("no answer in {} s", LIVE_CHECK.as_secs()),
        };
        let key = space.id().to_string();
        tracing::info!(space = %key, %reason, "its connection is dead; reconnecting");
        self.forget_stale_connection(&key).await;
        self.space(&key).await
    }

    /// Whether the relay lists offline the computer that carries Space
    /// `key`: the relay machine itself, or the host a routed Space rides
    /// on. False when no relay computer carries it or the relay has not
    /// listed it.
    pub(crate) fn carrier_listed_offline(&self, key: &str) -> bool {
        let machine = match SpaceId::parse(key) {
            Ok(SpaceId::Relay { machine_id }) => machine_id,
            Ok(SpaceId::Direct { authority }) => match crate::id::routed_host(&authority) {
                Some((machine, _)) => machine.to_string(),
                None => return false,
            },
            _ => return false,
        };
        matches!(self.relay_presence(&machine), Some((false, _)))
    }

    /// [`Spaces::resolve`], first asking where a durable Space is now when
    /// `space` is a name and the coordinator of your machines can be asked
    /// (a durable Space's name follows it from machine to machine).
    pub async fn resolve_now(&self, space: &str) -> Result<SpaceId> {
        if SpaceId::parse(space).is_err()
            && self.inner.pool.read().expect("pool").is_some()
            && let Err(e) = self.durable_spaces().await
        {
            tracing::debug!(error = %e, "durable Spaces not refreshed");
        }
        self.resolve(space)
    }

    /// Drops the cached connection so the next [`Spaces::space`] reconnects
    /// (and re-reads capabilities).
    pub async fn forget_connection(&self, space: &str) -> Result<()> {
        let id = self.resolve(space)?;
        // What an extension attached over its own socket (a mounted volume,
        // where an agent's home may be) stays; the next connection tells
        // the extensions again.
        self.forget_stale_connection(&id.to_string()).await;
        // Asked for on purpose: dial at once, whatever failed before.
        self.inner.dial_backoff.clear(&id.to_string());
        Ok(())
    }

    async fn connect(
        &self,
        id: &SpaceId,
        credential: &Credential,
        name: Option<&str>,
        record: Option<&dpb::Space>,
    ) -> Result<Space> {
        self.connect_with(id, credential, name, record, self.inner.probe_timeout)
            .await
    }

    /// Connects to a Space: its declared services, plus cua-spacesd when
    /// the image runs it. `probe` bounds the spacesd handshake. Without
    /// a spacesd, a Local Space (and a direct Space registered
    /// with service URLs) is still a Space, with an empty capability set;
    /// a direct URL with neither fails with [`Error::SpacesdNotAvailable`].
    /// `record` is the Space's registry record, read once by the caller.
    async fn connect_with(
        &self,
        id: &SpaceId,
        credential: &Credential,
        name: Option<&str>,
        record: Option<&dpb::Space>,
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
        let mut bulk_file: Option<std::path::PathBuf> = None;
        let mut routed_forward: Option<Arc<cua_spacesd_client::tunnel::TcpForward>> = None;
        // Why a relay connection fell back to the relay's HTTPS path.
        let mut fallback: Option<String> = None;
        // How to reach the spacesd, if the target can have one.
        let (options, gateway, route): (
            Option<ConnectOptions>,
            Option<Gateway>,
            Option<crate::ChannelRoute>,
        ) = match id {
            SpaceId::Direct { authority } if crate::id::routed_host(authority).is_some() => {
                // Provided by a host and reached through it: a forward of
                // its loopback port over the host's own channel.
                let (machine, port) = crate::id::routed_host(authority).expect("routed");
                let host = self.space_of_host(format!("relay:{machine}")).await?;
                let fwd = host
                    .spacesd()?
                    .forward_tcp(cua_spacesd_client::tunnel::ForwardOptions::new(port))
                    .await?;
                let mut o = ConnectOptions::parse(&format!("http://{}", fwd.local_addr()))?;
                o.token = credential.token.clone();
                routed_forward = Some(Arc::new(fwd));
                // Its host's connection carries it: the same transport.
                fallback = host.fallback_reason().map(str::to_owned);
                (Some(o), None, host.route().cloned())
            }
            SpaceId::Direct { authority } => {
                if credential.service_urls.is_empty() {
                    let url = credential
                        .url
                        .clone()
                        .unwrap_or_else(|| format!("http://{authority}"));
                    let mut o = ConnectOptions::parse(&url)?;
                    o.token = credential.token.clone();
                    // A machine on a self-hosted (static-token) relay: the
                    // mesh when the relay coordinates one (pairing with the
                    // env token), the relay's HTTPS path otherwise.
                    let mut route = Some(crate::ChannelRoute::Direct(o.endpoint.origin()));
                    if let (cua_spacesd_client::EndpointKind::Relay { machine_id }, Some(token)) =
                        (o.endpoint.kind().clone(), credential.token.as_deref())
                    {
                        let relay_url = o.endpoint.origin();
                        let via = crate::mesh::Via::Static {
                            relay: &relay_url,
                            token,
                        };
                        route = Some(crate::ChannelRoute::Relay(relay_url.clone()));
                        match crate::mesh::connect_options(
                            &self.inner.mesh,
                            crate::mesh::Via::Static {
                                relay: &relay_url,
                                token,
                            },
                            &machine_id,
                            crate::mesh::policy(),
                        )
                        .await?
                        {
                            crate::mesh::Route::Mesh(m, via_mesh) => {
                                tracing::info!(machine = %machine_id, "relay Space over the mesh");
                                self.inner.mesh.note_routed(&id.to_string(), &via, false);
                                o = *m;
                                route = Some(via_mesh);
                            }
                            crate::mesh::Route::Relay { fallback: why } => fallback = why,
                        }
                    }
                    (Some(o), None, route)
                } else {
                    // Added by an MCP URL: the token belongs to that server.
                    (None, None, None)
                }
            }
            // Cua Cloud has ended: a legacy `cloud:` Space cannot connect.
            SpaceId::Cloud { .. } => return Err(cua_sandbox_core::Error::CloudEnded.into()),
            SpaceId::Relay { machine_id } => {
                let relay = self.relay()?;
                // A Space one of your own machines provides prefers the mesh
                // (often a LAN neighbour); others follow `CUA_MESH`.
                // ... and so does a host whose routed Spaces ride its channel.
                let provided = self.provided_by_a_host(machine_id, record).await
                    || self.routes_through(machine_id);
                let policy = if provided {
                    crate::mesh::policy_for_provided()
                } else {
                    crate::mesh::policy()
                };
                // The WireGuard®-based mesh first (end to end, direct when
                // possible); the relay's HTTPS path when the relay or the
                // machine does not speak it yet (see `crate::mesh`).
                match crate::mesh::connect_options(
                    &self.inner.mesh,
                    crate::mesh::Via::Account(&relay),
                    machine_id,
                    policy,
                )
                .await?
                {
                    crate::mesh::Route::Mesh(o, route) => {
                        tracing::info!(machine = %machine_id, route = %route.describe(), "relay Space over the mesh");
                        self.inner.mesh.note_routed(
                            &id.to_string(),
                            &crate::mesh::Via::Account(&relay),
                            provided,
                        );
                        (Some(*o), None, Some(route))
                    }
                    crate::mesh::Route::Relay { fallback: why } => {
                        fallback = why;
                        if policy.uses_mesh() {
                            // Its driver may join the mesh a moment later:
                            // move there once it has (`Spaces::space`).
                            self.inner.mesh.note_fallback(
                                &id.to_string(),
                                &crate::mesh::Via::Account(&relay),
                                machine_id,
                            );
                        }
                        let url = cua_host::RelayClient::new(&relay.url)?.machine_url(machine_id);
                        // The account token rides in `authorization` and
                        // this device's session next to it, fetched (and
                        // refreshed) per call; the relay checks both and
                        // swaps them for its signed identity assertion.
                        let route = crate::ChannelRoute::Relay(relay.url.clone());
                        let bearer = crate::relay::RelayBearer { account: relay };
                        let mut o = ConnectOptions::parse(&url)?.gateway(Arc::new(bearer));
                        o.token = credential.token.clone();
                        (Some(o), None, Some(route))
                    }
                }
            }
            SpaceId::Local { name } => {
                let sandbox = self.inner.sandboxes.connect(name).await?;
                image = sandbox
                    .image_info()
                    .map(|i| (i.reference.clone(), i.digest.clone()));
                let placed = sandbox.placement().0;
                let runtime = sandbox.runtime_type().to_string();
                platform = sandbox.image_info().map(|i| {
                    let kind = kind_of_runtime(&runtime, &i.variant, placed);
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
                                source: ServiceSource::Sandbox(Box::new(service)),
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
                bulk_file = sandbox
                    .provider_details()
                    .get("bulk")
                    .map(std::path::PathBuf::from);
                match sandbox.port(env_port) {
                    Ok(PortTarget::Addr { host, port }) => {
                        let addr = authority(&host, port);
                        let mut o = ConnectOptions::parse(&format!("http://{addr}"))?;
                        o.token = token;
                        (Some(o), None, Some(crate::ChannelRoute::Local(addr)))
                    }
                    Ok(PortTarget::Url(u)) => {
                        let mut o = ConnectOptions::parse(&u)?;
                        o.token = token;
                        (Some(o), None, Some(crate::ChannelRoute::Local(u)))
                    }
                    // The image publishes no spacesd port.
                    Err(_) => (None, None, None),
                }
            }
        };
        let mut unavailable: Option<(String, String)> = None;
        let env = match options {
            None => {
                // No spacesd port is published. For a Space this computer
                // knows runs cua-spacesd (it answered before), that is a
                // runtime still bringing the Space back (a restart
                // publishes the ports a moment later), not an image
                // without it: mark it so the next use dials again.
                if matches!(id, SpaceId::Local { .. }) && record_runs_spacesd(record) {
                    unavailable = Some((
                        String::new(),
                        "the Space's cua-spacesd port is not published yet".into(),
                    ));
                }
                None
            }
            Some(options) => match SpacesdClient::connect(options.probe_timeout(probe)).await {
                Ok(env) => Some(env),
                Err(cua_spacesd_client::Error::SpacesdNotAvailable { endpoint, reason }) => {
                    let generic_ok = matches!(id, SpaceId::Local { .. }) || !services.is_empty();
                    if let SpaceId::Relay { machine_id } = &id
                        && crate::host_spaces::is_stopped_sharing(&reason)
                    {
                        // Not a missing driver: its owner stopped sharing it.
                        let named = name.filter(|n| !n.is_empty()).map(str::to_string);
                        let label = named.or_else(|| {
                            self.inner
                                .relay_cache
                                .lock()
                                .expect("relay cache")
                                .iter()
                                .find(|m| &m.id == machine_id && !m.name.is_empty())
                                .map(|m| m.name.clone())
                        });
                        return Err(crate::host_spaces::stopped_sharing(
                            label.as_deref().unwrap_or(&id.to_string()),
                        ));
                    }
                    if let SpaceId::Relay { .. } = &id
                        && let Some(message) = crate::host_spaces::team_device_refusal(&reason)
                    {
                        // Refused by the team, not a Space still starting.
                        return Err(crate::host_spaces::team_device_refused(message));
                    }
                    if !generic_ok {
                        return Err(Error::SpacesdNotAvailable {
                            space: id.to_string(),
                            reason,
                        });
                    }
                    tracing::debug!(space = %id, %reason, "no cua-spacesd; a Space of declared services");
                    unavailable = Some((endpoint, reason));
                    None
                }
                Err(e) => return Err(e.into()),
            },
        };
        // A Linux VM on this machine shares a disk with its guest: large
        // transfers go through it when the guest proves it is the same one.
        let mut bulk_proven = None;
        if let (Some(env), Some(file)) = (&env, bulk_file)
            && env.attach_bulk(&file).await
        {
            bulk_proven = Some(file);
        }
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
        if let Some(f) = routed_forward {
            let _ = space.inner.keepalive.set(f);
        }
        if let Some(route) = route.filter(|_| space.has_spacesd()) {
            if route.transport() == crate::Transport::RelayHttps
                && let Some(reason) = fallback
            {
                space.set_fallback_reason(reason);
            }
            space.set_route(route);
        }
        if let Some(file) = bulk_proven {
            space.set_bulk_path(file);
        }
        if let Some((reference, digest)) = image {
            space.set_image(reference, digest);
        }
        if let Some((kind, arch)) = platform {
            space.set_platform(kind, arch);
        }
        if let Some(record) = record
            && !record.image_config.is_empty()
            && let Ok(config) = serde_json::from_str(&record.image_config)
        {
            space.set_image_config(config);
        }
        if let Some((endpoint, reason)) = unavailable {
            space.set_spacesd_unavailable(endpoint, reason);
        }
        Ok(space)
    }

    /// Connects once the sandbox is ready. When the image is expected to run
    /// cua-spacesd, retries until the driver answers or `budget` runs out
    /// (the driver may start a moment after the port is published; bounded:
    /// at most `budget / 2s` attempts). Otherwise one bounded handshake, and
    /// a Space of declared services when no driver answers. `name` is the
    /// Space's name (without one, the guest's hostname).
    async fn connect_when_ready(
        &self,
        id: &SpaceId,
        credential: &Credential,
        name: Option<&str>,
        budget: Duration,
        expect_spacesd: bool,
    ) -> Result<Space> {
        let record = self.inner.registry.get(&id.to_string())?;
        let record = record.as_ref();
        if !expect_spacesd {
            let probe = self
                .inner
                .probe_timeout
                .min(budget.max(Duration::from_secs(1)));
            return self.connect_with(id, credential, name, record, probe).await;
        }
        let deadline = tokio::time::Instant::now() + budget;
        let max_attempts = (budget.as_secs() / 2).max(1) + 1;
        let mut last = None;
        for _ in 0..max_attempts {
            match self.connect(id, credential, name, record).await {
                Ok(s) if s.has_spacesd() => {
                    wait_serving(&s, deadline).await;
                    return Ok(s);
                }
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

    /// A registered Space in `provider` that is reachable and has what was
    /// asked for (the declared `services`; a healthy cua-spacesd when one is
    /// expected).
    async fn reusable(
        &self,
        provider: Provider,
        services: &BTreeMap<String, u16>,
        expect_env: bool,
    ) -> Result<Option<SpaceInfo>> {
        for info in self.list()? {
            if info.provider != provider {
                continue;
            }
            let Ok(space) = self.space(&info.id).await else {
                continue;
            };
            // Reuse only a Space that has what was asked for.
            if !services
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
                !services.is_empty()
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
        crate::registry::validate_labels(&opts.labels)?;
        // Under the operator override that turns the secure default off, a create asks for no
        // more than the old lists. Child Spaces is an experiment: until it is on, no parent.
        crate::network::require_secure_spaces(opts.network_settings.as_ref())?;
        // The network it gets: what it asks, no more open than its parent.
        // Every path below carries the resolved settings.
        let network = self.create_network(&opts)?;
        opts.network_settings = Some(network.settings.clone());
        let egress = opts.egress.clone();
        if !egress.is_empty() {
            // Checked before anything is created; the runtime refuses a
            // kind it cannot enforce one on, also before anything exists.
            crate::egress::check(&egress)?;
            if opts.durable || opts.reuse {
                return Err(Error::invalid(
                    "egress does not apply with durable or reuse: a durable Space is created \
                     again elsewhere without it, and a reused Space keeps the policy it has",
                ));
            }
            if opts.wait == Some(false) {
                // The policy is applied once the Space is ready; a create
                // that returns before then would hand back an open Space.
                return Err(Error::invalid(
                    "egress needs the Space ready: a create with wait = false returns before it \
                     is, so create it with wait, or set the policy with set_egress once it is",
                ));
            }
        }
        if opts.network == cua_sandbox_core::NetworkMode::None && (opts.durable || opts.reuse) {
            return Err(Error::invalid(
                "network=none does not apply with durable or reuse: a durable Space is created \
                 again elsewhere without it, and a reused Space keeps the network it has",
            ));
        }
        // A fork of a checkpoint, wherever the checkpoint is.
        if crate::pool_checkpoints::checkpoint_image(opts.image.as_deref()).is_some() {
            // Boxed: the fork's future is deep (the batch planner).
            return boxed(move || self.create_from_checkpoint(opts)).await;
        }
        if let Some(r) = opts
            .image
            .as_deref()
            .and_then(|i| i.strip_prefix(crate::build::BUILD_IMAGE_PREFIX))
        {
            let r = r.to_string();
            return boxed(|| self.create_from_build(&r, opts)).await;
        }
        if opts.durable {
            if !opts.labels.is_empty() {
                return Err(Error::invalid(
                    "labels do not apply to a durable Space: it is created again on another \
                     machine, which would not carry them",
                ));
            }
            return boxed(move || self.create_durable(opts)).await;
        }
        self.require_volumes(&opts.volumes)?;
        if !opts.volumes.is_empty() {
            // A typo in a volume name is an error here, not an empty folder.
            let names: Vec<&str> = opts.volumes.iter().map(|v| v.name.as_str()).collect();
            self.call_extension(
                "Cua Volume (volumes in Spaces)",
                "volumes.exists",
                serde_json::json!({ "names": names }),
            )
            .await?;
        }
        if opts.reuse && !opts.volumes.is_empty() {
            return Err(Error::invalid(
                "reuse does not apply with volumes: a reused Space has its own mounts",
            ));
        }
        if opts.reuse && !opts.labels.is_empty() {
            return Err(Error::invalid(
                "labels do not apply with reuse: a reused Space keeps the labels it was \
                 created with",
            ));
        }
        let sink = opts.progress.take().map(|p| p.0);
        let (key, on) = self.create_key(&mut opts)?;
        // A Mac cannot create a Windows VM locally yet. With the `windows-kvm`
        // experiment on, the selected remote host may be a Linux or Windows
        // machine that can run it, so the coordinator's local restriction
        // applies only to local placement. Off: it applies everywhere.
        if on == On::Local || !cua_vmm::experiments::enabled(cua_vmm::experiments::Gate::WindowsKvm)
        {
            refuse_windows_on_mac(opts.image.as_deref(), cfg!(target_os = "macos"))?;
        }
        // The record is written before the Space connects, and the volume
        // attaches on connect: its mounts are known by then.
        self.expect_volumes(&key.id, opts.volumes.clone());
        if !egress.is_empty()
            && let On::Provider(word) = &on
        {
            return Err(crate::egress::refused(
                word,
                "a Space in your own cloud account runs on a machine with no host-side \
                 point cua controls; create it on this machine or one of yours",
            ));
        }
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
        if opts.wait == Some(false) {
            // The whole create runs in the background (still cancellable);
            // the caller gets the id it will have.
            let flight = crate::creating::register(self.home_dir(), &key)?;
            let spaces = self.clone();
            let id = key.id.clone();
            opts.wait = Some(true);
            tokio::spawn(cua_sandbox_core::progress::carry(sink, async move {
                match boxed(|| spaces.create_tracked(opts, key, flight)).await {
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
            let created = boxed(move || self.create_tracked(opts, key, flight)).await?;
            report(Progress::phase(Phase::Ready));
            Ok(created)
        })
        .await
    }

    /// The GPU options each runtime of `on` offers here: on this machine,
    /// per local runtime (Lume: "GPU acceleration" for macOS VMs; QEMU:
    /// virgl on Linux; containers: NVIDIA on Linux).
    /// A runtime with no option says why ([`cua_sandbox_core::gpu`]).
    pub async fn gpu_support(&self, on: &On) -> Vec<cua_sandbox_core::gpu::GpuSupport> {
        let sbx = &self.inner.sandboxes;
        match on {
            On::Local => sbx.gpu_support(ProviderKind::Local, None).await,
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
        if on == On::Cloud {
            return Err(cua_sandbox_core::Error::CloudEnded.into());
        }
        let create_id = opts.create_id.clone().filter(|c| !c.trim().is_empty());
        // A name in the SDK's own form counts as generated: an app that
        // shows the name before creating picks it the same way
        // ([`is_generated_name`]).
        let generated = opts
            .name
            .as_deref()
            .is_none_or(|n| n.trim().is_empty() || is_generated_name(n.trim()));
        let named = |opts: &mut SpaceCreate| {
            let name = sanitize_label(
                &opts
                    .name
                    .clone()
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| self.fresh_name()),
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
        let kind = if key.stem.starts_with("other.") {
            "host"
        } else {
            "local"
        };
        let expect_env = expects_spacesd(opts.image.as_deref(), opts.spacesd);
        let labels = opts.labels.clone();
        let relay_off = opts.relay == Some(false)
            || labels
                .get(crate::registry::RELAY_LABEL)
                .is_some_and(|v| crate::registry::RelaySetting::parse(v).is_ok_and(|s| !s.is_on()));
        let joined_tree = opts.tree.clone().filter(|_| opts.parent.is_none());
        let egress = opts.egress.clone();
        let network = crate::network::CreateNetwork {
            settings: opts
                .network_settings
                .clone()
                .unwrap_or_else(crate::network::NetworkSettings::internet),
            parent: opts
                .parent
                .as_deref()
                .and_then(|p| self.resolve(p).ok())
                .map(|i| i.to_string()),
            root: None,
            depth: 0,
        };
        let journal =
            crate::creating::JournalHandle::start(self.home_dir(), &key, kind, expect_env);
        // A create dropped before it returned (a closed request, a Ctrl-C
        // the caller did not handle) is undone the same way.
        let mut guard = UndoOnDrop {
            spaces: self.clone(),
            journal: Some(journal.clone()),
        };
        // A Space made on this computer shows on your other computers while
        // it is created ([`crate::resources::creates`]): it leaves the list
        // when this returns, fails, is cancelled or is dropped.
        let on_board = (kind == "local").then(|| {
            let hub = self.inner.resources.clone();
            let os = opts
                .image
                .as_deref()
                .map(crate::elastic::guest_os)
                .unwrap_or_default();
            hub.creates.start(&key.id, &key.name, &os);
            crate::resources::creates::OnBoard {
                board: hub,
                id: key.id.clone(),
            }
        });
        let outer = cua_sandbox_core::progress::current();
        let sink: Option<cua_sandbox_core::progress::Sink> = on_board.as_ref().map(|b| {
            let (hub, id) = (b.board.clone(), b.id.clone());
            Arc::new(move |p: &CreateProgress| {
                hub.creates.report(&id, p);
                if let Some(outer) = &outer {
                    outer(p);
                }
            }) as cua_sandbox_core::progress::Sink
        });
        cua_sandbox_core::progress::carry(sink.clone(), async {
            cua_sandbox_core::progress::report(cua_sandbox_core::progress::Progress::phase(
                cua_sandbox_core::progress::Phase::Preparing,
            ));
        })
        .await;
        let wants_volumes = !opts.volumes.is_empty();
        let volumes = opts.volumes.clone();
        let run = Box::pin(crate::creating::scope(
            journal.clone(),
            Box::pin(async move {
                let created = self.create_inner(opts).await?;
                // Mounts were asked for: the Space is only ready once they
                // are in place (a Space that cannot have them is removed).
                if wants_volumes
                    && let SpaceCreated::Ready {
                        info,
                        reused: false,
                    } = &created
                {
                    // A Space on another machine (a pool host) has no record
                    // here: this machine, which holds the Cua Volume, serves
                    // the volume over the Space's own connection.
                    if !(info.id.starts_with("local:") || info.id.starts_with("cloud:")) {
                        self.inner
                            .registry
                            .set_overlay_volumes(&info.id, volumes.clone())?;
                    }
                    if let Err(e) = boxed(|| self.ensure_volumes(&info.id)).await {
                        let _ = boxed(|| self.delete(&info.id)).await;
                        return Err(e);
                    }
                }
                // Reported with the mounts it now has.
                Ok(match created {
                    SpaceCreated::Ready {
                        mut info,
                        reused: false,
                    } if wants_volumes => {
                        info.volumes = volumes;
                        SpaceCreated::Ready {
                            info,
                            reused: false,
                        }
                    }
                    other => other,
                })
            }),
        ));
        let run = cua_sandbox_core::progress::carry(sink, run);
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
        // What a create that did not get as far as a record leaves behind.
        self.expect_volumes(&key.id, vec![]);
        let mut result = result;
        if let Ok(created) = &mut result
            && !matches!(created, SpaceCreated::Ready { reused: true, .. })
        {
            // The Space exists: keep its labels and when it was created,
            // and close its network as asked. A Space that cannot be left
            // as asked for is deleted, not handed back.
            let id = match created {
                SpaceCreated::Ready { info, .. } => &info.id,
                SpaceCreated::Starting(p) => &p.id,
            }
            .clone();
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let kept = self.inner.registry.update_meta(&id, |m| {
                m.labels = labels.clone();
                m.created_at_unix = now;
                if relay_off {
                    m.relay = crate::registry::RelaySetting::Off;
                }
                // A member of a tree that started on another computer.
                if let Some(t) = &joined_tree {
                    m.overlay_tree = Some(t.clone());
                }
            });
            // The daemon lists it on the account without waiting for its tick.
            #[cfg(feature = "spaces-agents")]
            crate::share::PUBLISH_CHANGED.notify_waiters();
            // The policy was enforced from the guest's first instruction
            // (by this machine's runtime, or the host that created it).
            let applied = match &kept {
                Ok(()) if !egress.is_empty() => self.inner.registry.update_meta(&id, |m| {
                    m.egress = egress.clone();
                }),
                _ => Ok(()),
            };
            let applied = applied.and_then(|()| {
                // Its network settings and its place in a tree.
                let root = network.parent.as_ref().map(|p| {
                    self.inner
                        .registry
                        .meta(p)
                        .and_then(|m| m.root)
                        .unwrap_or_else(|| p.clone())
                });
                let depth = network.parent.as_ref().map_or(0, |p| {
                    self.inner.registry.meta(p).map_or(0, |m| m.depth) + 1
                });
                self.record_network(
                    &id,
                    &crate::network::CreateNetwork {
                        root,
                        depth,
                        ..network.clone()
                    },
                )
            });
            if let Some(e) = kept.err().or(applied.err()) {
                let _ = Box::pin(self.delete(&id)).await;
                result = Err(e);
            } else if let Ok(SpaceCreated::Ready { info, .. }) = &mut result {
                info.labels = labels.clone();
                info.hidden = labels.contains_key(crate::registry::HIDDEN_LABEL);
                info.created_at_unix = now;
                info.egress = egress.clone();
                info.egress_enforced =
                    (!egress.is_empty()).then_some(crate::egress::Enforcement::Host);
                info.network = Some(crate::network::SpaceNetwork {
                    settings: network.settings.clone(),
                    note: None,
                    notice_pending: false,
                });
            }
        }
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

    /// The create, off the caller's frame. A named return type, not an
    /// `async fn`: the body awaits this again (the overflow retry), and an
    /// opaque future cannot be asked for `Send` while it is being defined.
    pub(crate) fn create_inner(
        &self,
        opts: SpaceCreate,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<SpaceCreated>> + Send + '_>>
    {
        boxed(move || self.create_inner_body(opts))
    }

    async fn create_inner_body(&self, mut opts: SpaceCreate) -> Result<SpaceCreated> {
        use cua_sandbox_core::placement;
        let omitted = opts.on.is_none();
        // A child made on another of your computers joins its parent's tree
        // there: the computer gets the tree's id, qualified with this one.
        if opts.tree.is_none()
            && !matches!(opts.on, None | Some(On::Local))
            && let Some(root) = self.tree_root_of(opts.parent.as_deref())
        {
            opts.tree = Some(cua_vmm::tree::qualify(&cua_vmm::tree::overlay_id(), &root));
        }
        let (on, source) = match opts.on.clone() {
            Some(on) => (on, cua_sandbox_core::settings::Source::Explicit),
            None => cua_sandbox_core::settings::Settings::load()
                .and_then(|s| s.default_on())
                .map_err(|e| Error::invalid(e.to_string()))?,
        };
        // No location given and none configured: this machine first, then
        // the linked clouds when it is full and a budget is set
        // (`placement.overflow`). An explicit `local` never overflows.
        if may_overflow(omitted, &on, &source, opts.reuse, !opts.mounts.is_empty())
            && self.overflow_allowed().await
        {
            let mut local = opts.clone();
            local.on = Some(On::Local);
            return match self.create_inner(local).await {
                Err(e) if overflows(&e) => {
                    tracing::info!("this machine is full ({e}); overflowing into your clouds");
                    boxed(move || self.create_overflow(opts))
                        .await
                        .map_err(|c| match c {
                            Error::LimitExceeded(why) => Error::LimitExceeded(format!(
                                "this machine is full and no cloud machine fits the budget: {why}"
                            )),
                            other => other,
                        })
                }
                r => r,
            };
        }
        let place = |e: placement::PlacementError| {
            Error::Sandbox(cua_sandbox_core::Error::InvalidPlacement(e))
        };
        if !opts.mounts.is_empty() && on != On::Local {
            return Err(Error::invalid(
                "mounts are directories of this computer: only a Space on local takes them",
            ));
        }
        if on == On::Auto {
            if opts.reuse {
                return Err(Error::invalid(
                    "reuse does not apply to on=\"auto\"; list Spaces with list_spaces",
                ));
            }
            placement::validate(&on, opts.kind, &opts.runtime).map_err(place)?;
            // With a linked cloud that supplies the pool: one solve over
            // your devices, your hosts and your clouds ([`crate::elastic`]).
            // Without one: the least-loaded of your machines, by what their
            // Spaces reserve (idle capacity is reclaimed before a refusal).
            if self.elastic_has_clouds().await {
                return boxed(move || self.create_on_auto_elastic(opts)).await;
            }
            return boxed(move || self.create_on_auto(opts)).await;
        }
        if let On::Host(query) = &on {
            if opts.reuse {
                return Err(Error::invalid(
                    "reuse does not apply to host: Spaces; list them with list_spaces",
                ));
            }
            placement::validate(&on, opts.kind, &opts.runtime).map_err(place)?;
            let info = boxed(move || self.create_on_host(query, opts)).await?;
            return Ok(SpaceCreated::Ready {
                info,
                reused: false,
            });
        }
        // A linked cloud that supplies the elastic pool (hosts, or Spaces
        // priced per Space): the same solve as on="auto", pinned to it.
        // With autoscale off you manage machines yourself: a cloud named
        // gets a machine of its own for this Space, as before the pool.
        if let On::Provider(word) = &on
            && !opts.reuse
            && self.elastic_policy().autoscale
            && self.elastic_supplier(word).await
        {
            return boxed(move || self.create_on_auto_elastic(opts)).await;
        }
        if let On::Provider(word) = &on
            && match self.inner.sandboxes.contrib_provider(word) {
                Ok(p) => p.joins_relay(),
                // A cloud-only word this build lacks says how to get it.
                Err(_) => !cua_sandbox_core::is_contrib_location(word),
            }
        {
            // Your own cloud account (`aws`, `gcp`, `hetzner`, `modal`): a sandbox there
            // whose driver joined the relay.
            // A cloud that keeps network rules itself (Modal) takes
            // network=none; any other refuses it here, before anything is made.
            if opts.network == cua_sandbox_core::NetworkMode::None
                && !self
                    .inner
                    .sandboxes
                    .contrib_provider(word)
                    .is_ok_and(|p| p.takes_network_rules())
            {
                return Err(Error::invalid(format!(
                    "network=none applies to VMs on your computers and to Modal, not Spaces in \
                     this cloud ({word})"
                )));
            }
            if opts.reuse {
                return Err(Error::invalid(format!(
                    "reuse does not apply to Spaces in your cloud ({word}); list them with \
                     list_spaces"
                )));
            }
            // `create_in_cloud` keeps the Space's network or refuses it.
            let info = boxed(move || self.create_in_cloud(word, opts)).await?;
            return Ok(SpaceCreated::Ready {
                info,
                reused: false,
            });
        }
        if on.is_existing_machine() {
            return Err(Error::invalid(format!(
                "{on} is an existing computer: add it with add_space (url {}), create_space \
                 makes a new one (on local or cloud)",
                on.to_string().split_once(':').map_or("", |(_, a)| a)
            )));
        }
        placement::validate(&on, opts.kind, &opts.runtime).map_err(place)?;
        let expect_env = expects_spacesd(opts.image.as_deref(), opts.spacesd);
        let provider = match &on {
            On::Local => Provider::Local,
            // Cua Cloud has ended ([`Spaces::create`] refuses it up front).
            On::Cloud => return Err(cua_sandbox_core::Error::CloudEnded.into()),
            other => {
                return Err(Error::HostCapabilityMissing {
                    what: format!("provider {}", other.location()),
                    why: "this build has no Spaces backend for it".into(),
                });
            }
        };
        if opts.reuse
            && let Some(info) =
                boxed(|| self.reusable(provider, &opts.services, expect_env)).await?
        {
            return Ok(SpaceCreated::Ready { info, reused: true });
        }
        let name = sanitize_label(&opts.name.unwrap_or_else(|| self.fresh_name()));
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
            mounts: opts.mounts,
            headless: opts.headless,
            forkable: opts.forkable,
            network: opts.network,
            egress: opts.egress,
            network_tree: self
                .tree_root_of(opts.parent.as_deref())
                .or_else(|| opts.tree.clone()),
            network_settings: opts.network_settings,
        };
        // `create` runs a `wait = false` create in the background
        // itself, so here it always waits.
        Ok(SpaceCreated::Ready {
            info: boxed(move || self.provision_local(local)).await?,
            reused: false,
        })
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
        let name = sanitize_label(&opts.name.clone().unwrap_or_else(|| self.fresh_name()));
        let expect_env = expects_spacesd(opts.image.as_deref(), opts.spacesd);
        // An alias (`linux`, `macos:26`, `macos:26-slim`) names a canonical
        // image, as it does for a cloud or host Space; the local runtime
        // only knows registry references (it would try to pull `linux`).
        let image = opts
            .image
            .as_deref()
            .map(|i| cua_image::canonical::alias(i).unwrap_or_else(|| i.to_string()))
            .unwrap_or_else(|| cua_image::canonical::canonical_image_or_linux("linux"));
        // A name that already has a sandbox keeps the token that sandbox was
        // started with: a new one would be recorded but never reach the
        // guest, and the existing Space would answer UNAUTHENTICATED to its
        // own record.
        let existing_token = self.inner.sandboxes.recorded_env_token(&name);
        let token = existing_token.unwrap_or_else(|| format!("{:032x}", rand::random::<u128>()));
        // Until the Space is registered, the create's journal holds its
        // token and what it made: a process that dies mid-create (a daemon
        // crash, a quit) leaves it, and [`Spaces::recover_interrupted_creates`]
        // then recovers it or retains it for manual cleanup; an explicit
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
        create.cpus = opts.cpus.unwrap_or(crate::local_load::DEFAULT_CPUS);
        create.memory_mb = opts
            .memory_mb
            .unwrap_or(crate::local_load::DEFAULT_MEMORY_MB);
        // Until the sandbox exists, other creates count this one's size.
        let _reserved = crate::local_load::Pending::hold(
            self.home_dir(),
            &name,
            create.kind != Kind::Vm && crate::host_spaces::resolve_image(&image).1 != "macos",
            create.cpus,
            create.memory_mb,
        );
        create.disk_gb = opts.disk_gb.filter(|g| *g > 0);
        create.env = opts.env.clone();
        create.env.insert("CUA_ENV_TOKEN".into(), token.clone());
        create.env_token = Some(token.clone());
        create.ready_timeout = timeout;
        create.ready_timeout_given = opts.timeout.is_some();
        create.command = opts.command.clone().filter(|c| !c.is_empty());
        create.gpu = opts.gpu.clone().filter(|g| !g.trim().is_empty());
        create.mounts = opts.mounts.clone();
        create.headless = opts.headless;
        create.forkable = opts.forkable;
        create.spacesd = opts.spacesd;
        create.network = opts.network;
        create.egress = opts.egress.clone();
        create.network_settings = opts.network_settings.clone();
        // Its tree: its parent's, or its own.
        create.network_tree = Some(
            opts.network_tree
                .clone()
                .unwrap_or_else(|| SpaceId::Local { name: name.clone() }.to_string()),
        );
        for (svc, port) in opts.services.iter().filter(|(s, _)| s.as_str() != "env") {
            create = create
                .service(svc.clone(), *port)
                .wait_for(Probe::Tcp(*port));
        }
        if expect_env {
            create = create.wait_for(Probe::Tcp(cua_proto::SPACESD_DEFAULT_PORT));
        }
        let started = std::time::Instant::now();
        // Cancel-safe: dropped (a cancelled create), it stops and deletes
        // what it made (its own token never fires; the create's does).
        let (created, image_ready) = cua_sandbox_core::progress::past(
            cua_sandbox_core::progress::Phase::Pulling,
            self.inner
                .sandboxes
                .create_cancellable(create, cua_sandbox_core::CancellationToken::new()),
        )
        .await;
        // A runtime that can't protect it yet started it without network
        // (or the user chose No protection): say so on the Space.
        if let Ok(sb) = &created {
            let settings = opts
                .network_settings
                .clone()
                .unwrap_or_else(crate::network::NetworkSettings::internet);
            let note = crate::network::note_for(sb.runtime_type(), &settings);
            let id = SpaceId::Local { name: name.clone() }.to_string();
            if let Err(e) = self
                .inner
                .registry
                .update_meta(&id, |m| m.network_note = note)
            {
                tracing::warn!(space = %id, error = %e, "could not record the Space's network note");
            }
        }
        if let Err(e) = created {
            // A start that failed after the instance existed (a readiness
            // probe that never passed, a host with no room) leaves a VM no
            // registry lists: nothing would ever show or stop it, and it
            // would hold this machine's memory and CPUs against every retry.
            // `fresh` says the name had nothing before this create, so
            // whatever is under it is this attempt's: delete it, whoever
            // chose the name.
            if fresh {
                match self.inner.sandboxes.delete(&name).await {
                    Ok(()) | Err(cua_sandbox_core::Error::NotFound(_)) => {}
                    Err(d) => {
                        tracing::warn!(sandbox = %name, error = %d, "could not delete a Space that failed to start")
                    }
                }
            }
            return Err(e.into());
        }
        // An image without cua-spacesd that the runtime gave one: the
        // create waited for it, and the connection expects it.
        let expect_env = expect_env
            || created
                .as_ref()
                .ok()
                .and_then(|sb| sb.image_info())
                .is_some_and(|i| i.injected);
        let id = SpaceId::Local { name: name.clone() };
        let credential = Credential {
            url: None,
            token: Some(token),
            ..Default::default()
        };
        let remaining = connect_budget(opts.timeout, image_ready.unwrap_or(started));
        cua_sandbox_core::progress::report(cua_sandbox_core::progress::Progress::phase(
            cua_sandbox_core::progress::Phase::Connecting,
        ));
        let space = match self
            .connect_when_ready(&id, &credential, Some(&name), remaining, expect_env)
            .await
        {
            Ok(s) => s,
            Err(e) => {
                if fresh && let Ok(sb) = self.inner.sandboxes.connect(&name).await {
                    let _ = sb.delete().await;
                }
                return Err(e);
            }
        };
        // A container Space reads the repo cache through a read-only mount:
        // its git gets the helper (best effort, bounded, silent).
        let guest_os = crate::host_spaces::resolve_image(&image).1;
        if opts.kind != Kind::Vm
            && opts.kind != Kind::MicroVm
            && guest_os != "macos"
            && guest_os != "windows"
        {
            crate::repo_cache::install(&space, &name).await;
        }
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

    /// Forks local checkpoint (or sandbox) `source` into a new Space on
    /// this machine, named `name` (default `<source>-<hex>`), and registers
    /// it once its cua-spacesd answers with the fork's own token (a fork
    /// that never answers is deleted). The one fork path: [`Spaces::fork`],
    /// the daemon's `ForkSpace`, batched forks ([`crate::fork_batch`]) and
    /// forks a host makes for other machines
    /// (`CreateHostSpaceRequest.fork_source`) all end here.
    pub async fn fork_local(&self, source: &str, name: Option<String>) -> Result<SpaceInfo> {
        // A local Space's id (`local:<name>`) names its sandbox.
        let source = match SpaceId::parse(source) {
            Ok(SpaceId::Local { name }) => name,
            _ => source.to_string(),
        };
        let source = source.as_str();
        let name = sanitize_label(
            &name
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| format!("{source}-{}", random_suffix())),
        );
        // A sandbox or checkpoint of that name wins; failing that, the name
        // may be a build's tag (or its key).
        let mut built_from: Option<String> = None;
        let sandbox = match self.inner.sandboxes.fork(source, &name).await {
            Err(cua_sandbox_core::Error::NotFound(m)) => match self.build_checkpoint(source) {
                Some(checkpoint) => {
                    built_from = Some(source.to_string());
                    self.inner.sandboxes.fork(&checkpoint, &name).await?
                }
                None => return Err(cua_sandbox_core::Error::NotFound(m).into()),
            },
            other => other?,
        };
        // The final layer of a build, forked by its checkpoint name, has the
        // build's configuration too.
        if built_from.is_none() && source.starts_with("build-") {
            built_from = Some(source.to_string());
        }
        let token = sandbox.env_token();
        let id = SpaceId::Local { name: name.clone() };
        // A fork mounts what its source does (its guest memory still has the
        // source's mount; the volume is attached to the fork again).
        if let Ok(mounts) = self.volume_mounts(&format!("local:{source}")) {
            self.expect_volumes(&id.to_string(), mounts);
        }
        let credential = Credential {
            url: None,
            token: token.clone(),
            ..Default::default()
        };
        let space = match self
            .connect_when_ready(
                &id,
                &credential,
                Some(&name),
                Duration::from_secs(120),
                token.is_some(),
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                let _ = sandbox.delete().await;
                return Err(e);
            }
        };
        // The fork is booted: a failure to register it must not leave it
        // running for a retry to duplicate.
        let registered = (|| -> Result<()> {
            self.store(&space, credential)?;
            if let Some(config) = built_from.as_deref().and_then(|b| self.build_config(b)) {
                space.set_image_config(config.clone());
                self.record_image_config(&id.to_string(), &config)?;
            }
            Ok(())
        })();
        teardown_on_error(registered, "fork", &name, sandbox.delete()).await?;
        // A fork starts with its source Space's labels and egress policy (a
        // checkpoint source has none).
        let _ = self.inherit_meta(
            &SpaceId::Local {
                name: source.to_string(),
            }
            .to_string(),
            &id.to_string(),
        );
        self.inner
            .connections
            .lock()
            .await
            .insert(id.to_string(), space);
        #[cfg(feature = "mcp")]
        self.notify_connected(&id);
        self.info(&id)
    }

    /// Recovers local creates left by a dead process when their cua-spacesd
    /// authenticates within `budget`. Failed, cancelled or borrowed local
    /// attempts retain their resources and journals for explicit cleanup;
    /// a journal whose sandbox the local runtime says is gone (and that no
    /// state file records) has nothing left to inspect and is removed.
    /// Live processes are left alone; nonlocal creates retain cancellation
    /// cleanup. The daemon runs this at startup.
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
            if j.kind != "local" {
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
            let unregistered = || {
                self.inner
                    .registry
                    .get(&id.to_string())
                    .ok()
                    .flatten()
                    .is_none()
            };
            let outcome = if !j.name.is_empty()
                && unregistered()
                && self.inner.sandboxes.local_name_gone(&j.name).await
            {
                RecoveryOutcome::NothingLeft
            } else if crate::creating::cancel_marked(&home, &stem)
                || local_made == Some(false)
                || j.token.trim().is_empty()
                || !j.spacesd
            {
                RecoveryOutcome::Failed(
                    "local attempt retained; inspect and clean up manually".into(),
                )
            } else if self
                .inner
                .registry
                .get(&id.to_string())
                .ok()
                .flatten()
                .is_some()
            {
                RecoveryOutcome::AlreadyRegistered
            } else {
                let credential = Credential {
                    token: Some(j.token.clone()),
                    ..Default::default()
                };
                match self
                    .connect_when_ready(&id, &credential, Some(&j.name), budget, j.spacesd)
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
                        Err(e) => RecoveryOutcome::Failed(format!(
                            "{e}; local attempt retained for manual cleanup"
                        )),
                    },
                    Err(e) => RecoveryOutcome::Failed(format!(
                        "{e}; local attempt retained for manual cleanup"
                    )),
                }
            };
            tracing::info!(space = %id, ?outcome, "interrupted create");
            if !matches!(outcome, RecoveryOutcome::Failed(_)) {
                crate::creating::remove(&home, &stem);
            }
            out.push(RecoveredCreate {
                id: id.to_string(),
                outcome,
            });
        }
        out
    }

    /// Deletes a Space's sandbox and forgets it: a cloud Space's sandbox is
    /// deleted (metering stops), a local one's instance is deleted. A Space
    /// added by address (`direct:`, `relay:`) is only forgotten: cua did not
    /// create it. Hotspots on it stop. A Space that registered itself on
    /// the relay (`relay_register`, or a share) leaves the relay directory
    /// with it, and deleting such a Space's `relay:<machine>` removes that
    /// record (as its owner) even when the Space itself is already gone.
    ///
    /// Its child Spaces ([`crate::tree`]) go first: see
    /// [`Spaces::delete_tree`], which can keep them instead.
    pub async fn delete(&self, space: &str) -> Result<String> {
        self.delete_tree(space, false).await
    }

    /// [`Spaces::delete`] of this Space alone (its children, if any, are
    /// the caller's).
    pub(crate) async fn delete_this(&self, space: &str) -> Result<String> {
        // A durable Space goes for good (deleting only its current copy
        // would have it created again).
        if self.inner.pool.read().expect("pool").is_some() {
            let _ = self.durable_spaces().await;
            let key = self
                .resolve(space)
                .map(|id| id.to_string())
                .unwrap_or_else(|_| space.to_string());
            if let Some((name, _)) = self.durable_of(space).or_else(|| self.durable_of(&key)) {
                return self.delete_durable(&name).await;
            }
        }
        self.delete_copy(space).await
    }

    /// [`Spaces::delete`] of this Space only, even when it is a durable
    /// Space's copy: what the coordinator that keeps durable Spaces uses to
    /// remove a copy it replaced or deleted.
    pub async fn delete_copy(&self, space: &str) -> Result<String> {
        #[cfg(feature = "spaces-stream")]
        self.drop_stream_entries(space);
        let id = match self.resolve(space) {
            Ok(id) => id,
            // A local sandbox no Space record names (a `cua sb fork`, a
            // sandbox made with `cua sb create`): delete it by name too.
            Err(Error::NotFound(m)) => match self.unregistered_local(space) {
                Some(id) => id,
                None => return Err(Error::NotFound(m)),
            },
            Err(e) => return Err(e),
        };
        let ended = self.delete_copy_of(id.clone()).await;
        if ended.is_ok() {
            // A Space made later under the same name starts without the
            // lifetime this one had.
            let _ = crate::lifetimes::forget(self.home_dir(), &id.to_string());
        }
        ended
    }

    /// `local:<name>` when `name` is a plain name (no `kind:` prefix) of a
    /// sandbox Cua made on this computer (a recorded one) that no Space
    /// record names. Only recorded sandboxes: a VM the runtime has that Cua
    /// never made (another tool's Lume VM) is never deleted by name here.
    fn unregistered_local(&self, name: &str) -> Option<SpaceId> {
        if name.is_empty() || name.contains(':') || name.contains('/') {
            return None;
        }
        self.inner
            .sandboxes
            .state()
            .load(name)
            .is_some()
            .then(|| SpaceId::Local {
                name: name.to_string(),
            })
    }

    async fn delete_copy_of(&self, id: SpaceId) -> Result<String> {
        self.learn_usage_before_delete(&id.to_string()).await;
        if let SpaceId::Local { name } = &id {
            if let Ok(space) = self.space(&id.to_string()).await {
                crate::repo_cache::collect(&space, name).await;
            }
            crate::repo_cache::forget(name);
        }
        // A Space added by address keeps running: if it is attached to the
        // relay, its driver is asked to leave while it is still reachable.
        #[cfg(feature = "spaces-agents")]
        if matches!(id, SpaceId::Direct { .. }) {
            self.detach_attached_space(&id.to_string()).await;
        }
        let message = self.delete_space(&id).await?;
        // What lived in the Space (its persistent agents) ends with it.
        #[cfg(feature = "mcp")]
        for e in &self.inner.extensions {
            e.space_deleted(self, &id.to_string()).await;
        }
        // A Space this device attached to the relay as a machine of its own
        // takes that machine with it.
        #[cfg(feature = "spaces-agents")]
        if !matches!(id, SpaceId::Relay { .. })
            && let Some(note) = self.release_attached(&id.to_string()).await
        {
            return Ok(format!("{message} Note: {note}."));
        }
        Ok(message)
    }

    async fn delete_space(&self, id: &SpaceId) -> Result<String> {
        let id = id.clone();
        self.drop_connection(&id).await;
        self.inner.thumbnails.remove(&id.to_string());
        // A Space in your cloud: its sandbox goes, with everything the
        // sandbox layer made for it (instance, relay machine).
        if let Some(sandbox) = self.cloud_sandbox_of(&id.to_string())? {
            self.inner.sandboxes.delete(&sandbox).await?;
            let _ = self.inner.registry.remove(&id.to_string());
            if let SpaceId::Relay { machine_id } = &id {
                self.relay_forget(machine_id);
            }
            return Ok(format!("Deleted {id} (sandbox {sandbox} in your cloud)."));
        }
        let what = match &id {
            // Cua Cloud has ended: a legacy `cloud:` Space has nothing to
            // delete. `remove` forgets its record.
            SpaceId::Cloud { .. } => return Err(cua_sandbox_core::Error::CloudEnded.into()),
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
                    "Removed {id} (added by address, so only forgotten; the computer is untouched)."
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
                    self.relay_forget(machine_id);
                    return Ok(message);
                }
                match row.as_ref().and_then(|m| m.host.clone().map(|h| (m, h))) {
                    Some((m, host)) => {
                        let message = self.delete_on_host(m, &host).await?;
                        let _ = self.inner.registry.remove(&id.to_string());
                        self.relay_forget(machine_id);
                        return Ok(message);
                    }
                    // A Space that registered itself as a machine of yours:
                    // its record leaves the relay directory.
                    None => match row {
                        Some(m) if m.role == "owner" && crate::relay::is_registered_space(&m) => {
                            self.forget_relay_machine(machine_id, false).await?;
                            format!("Removed {id} from your relay directory.")
                        }
                        Some(m) if m.role != "owner" => format!(
                            "Removed {id} (disconnected; it is shared with you, so only its \
                             owner can remove it from the relay directory)."
                        ),
                        Some(_) => format!(
                            "Removed {id} (disconnected; it is one of your computers, so it stays \
                             in your relay directory: run `cua host remove` on it, or if that \
                             computer is gone, `cua spaces relay-unregister {id}`)."
                        ),
                        None => format!("Removed {id} (disconnected)."),
                    },
                }
            }
        };
        self.inner.registry.remove(&id.to_string())?;
        Ok(what)
    }

    /// The relay directory row of `machine_id`, or `None` when it is not listed.
    pub(crate) async fn relay_row(&self, machine_id: &str) -> Option<crate::relay::RelayMachine> {
        self.relay_directory(Some(machine_id))
            .await
            .ok()?
            .into_iter()
            .find(|m| m.id == machine_id)
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
        // A Space whose capture failed is not asked again until its
        // backoff passes (a host that does not share its desktop refuses
        // every capture; asking each refresh only fills its access log).
        if let Some(f) = cache.backing_off(&id, std::time::Instant::now()) {
            return cached.ok_or_else(|| thumbnail_failure(&f));
        }
        match self.capture_thumbnail(&id).await {
            Ok(t) => Ok(t),
            Err(e) => cached.ok_or(e),
        }
    }

    /// Captures and caches the Space's thumbnail now; a failure starts the
    /// Space's backoff ([`crate::thumbnails::ThumbnailCache::fail`]).
    async fn capture_thumbnail(&self, id: &str) -> Result<crate::thumbnails::Thumbnail> {
        let result = self.capture_thumbnail_once(id).await;
        if let Err(e) = &result {
            let denied = matches!(
                e,
                Error::Env(cua_spacesd_client::Error::PermissionDenied(_))
                    | Error::Env(cua_spacesd_client::Error::FeatureUnsupported { .. })
                    | Error::CapabilityMissing { .. }
            );
            self.inner
                .thumbnails
                .fail(id, std::time::Instant::now(), denied, e.to_string());
        }
        result
    }

    async fn capture_thumbnail_once(&self, id: &str) -> Result<crate::thumbnails::Thumbnail> {
        use crate::thumbnails::{MAX_DIMENSION, QUALITY, Thumbnail};
        let space = self.space(id).await?;
        // A machine that does not share its desktop says so in its
        // capabilities: never ask its ComputerService (each refusal is a
        // line in its owner's access log). The refusal still backs off.
        if let Some(f) = crate::thumbnails::desktop_not_shared(space.capabilities()) {
            return Err(Error::CapabilityMissing {
                space: id.to_string(),
                feature: f.name.clone(),
                limitation: f.limitation.clone(),
            });
        }
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
            if off
                || info.spacesd_version.is_empty()
                || !cache.due(&info.id, now)
                || cache
                    .backing_off(&info.id, std::time::Instant::now())
                    .is_some()
            {
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

    /// Whether the local Space `id` is running now (its sandbox's power
    /// state, from state files only). The state is the second half of
    /// [`Self::power_of`]; the first is how it is turned off.
    pub fn is_running(&self, id: &str) -> bool {
        self.power_of(id).1 == "running"
    }

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

    /// [`SpaceInfo::stop_reason`] of the Space `id`: why its local sandbox
    /// stopped without being asked to, in plain words, from its state file
    /// (empty when it did not, or is not local).
    pub fn stop_reason_of(&self, id: &str) -> String {
        let Ok(SpaceId::Local { name }) = SpaceId::parse(id) else {
            return String::new();
        };
        self.inner
            .sandboxes
            .stop_reason(&name)
            .map(|r| r.words())
            .unwrap_or_default()
    }

    /// Why the local Space `id` stopped without being asked to, looked at
    /// now (its engine is asked, never started): `None` while it runs, for
    /// a Space that is not local, or one that was stopped on purpose. Unlike
    /// [`Self::stop_reason_of`] this does not wait for the next stop watch.
    pub async fn unasked_stop_of(&self, id: &str) -> Option<String> {
        let Ok(SpaceId::Local { name }) = SpaceId::parse(id) else {
            return None;
        };
        self.inner
            .sandboxes
            .unasked_stop(&name)
            .await
            .map(|r| r.words())
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
    /// instance stops). Streams and hotspots on it end first. A Space added
    /// by address and your own computers on the relay cannot be turned off,
    /// and a legacy `cloud:` Space fails with
    /// [`cua_sandbox_core::CLOUD_ENDED`].
    pub async fn stop(&self, space: &str) -> Result<SpacePower> {
        #[cfg(feature = "spaces-stream")]
        self.drop_stream_entries(space);
        // Its child Spaces first ([`crate::tree`]).
        self.power_off_children(space, false).await;
        self.set_power(space, false).await
    }

    /// Turns a Space on again: resumes a suspended one, boots a stopped one
    /// (and, for a Space one of your machines provides, attaches it to the
    /// relay again), and leaves a running one as it is. Returns once it
    /// answers again (bounded).
    pub async fn start(&self, space: &str) -> Result<SpacePower> {
        // The runtime keeps an egress policy with the Space: it applies
        // again before the guest's first instruction. A Space from before
        // network settings moves to them now ([`crate::network`]).
        if let Ok(id) = self.resolve(space)
            && let Err(e) = self.migrate_network(&id.to_string())
        {
            tracing::warn!(space, error = %e, "could not record the Space's network settings");
        }
        let power = self.set_power(space, true).await?;
        // Its child Spaces after it ([`crate::tree`]).
        self.power_on_children(space).await;
        Ok(power)
    }

    /// Suspends a local Space, keeping its memory (it resumes where it
    /// was, and its memory stays in use meanwhile): a container or a QEMU
    /// VM. [`Self::stop`] stops it instead and frees its memory. Other
    /// Spaces are refused.
    pub async fn suspend(&self, space: &str) -> Result<SpacePower> {
        // Its child Spaces first ([`crate::tree`]).
        self.power_off_children(space, true).await;
        self.suspend_one(space).await
    }

    /// [`Spaces::suspend`] of this Space alone.
    pub(crate) async fn suspend_one(&self, space: &str) -> Result<SpacePower> {
        #[cfg(feature = "spaces-stream")]
        self.drop_stream_entries(space);
        let id = self.resolve(space)?;
        let SpaceId::Local { name } = &id else {
            return Err(Error::WrongProvider {
                op: "Suspending".into(),
                provider: format!("{} (only a Space on this computer)", id.provider()),
            });
        };
        let sandboxes = &self.inner.sandboxes;
        if sandboxes.power_control(name).is_none() {
            return Err(Error::invalid(format!("{space} cannot be suspended")));
        }
        self.drop_connection(&id).await;
        let state = sandboxes
            .power_off_as(name, cua_sandbox_core::PowerControl::Suspend)
            .await?;
        Ok(SpacePower::new(
            &id.to_string(),
            cua_sandbox_core::PowerControl::Suspend,
            state,
        ))
    }

    /// Gives a local VM Space another shape: `cpus` vCPUs, `memory_mb` MiB
    /// of memory, a disk grown to `disk_gb` GiB (each `None` keeps it). The
    /// VM shuts down and boots again from its own disk (a stopped one
    /// boots), so its files stay and its memory and processes do not.
    /// Returns once it is ready again. Other Spaces are refused.
    pub async fn resize(
        &self,
        space: &str,
        cpus: Option<u32>,
        memory_mb: Option<u64>,
        disk_gb: Option<u32>,
    ) -> Result<SpaceResize> {
        if cpus.is_none() && memory_mb.is_none() && disk_gb.is_none() {
            return Err(Error::invalid(
                "say what to change: cpus, memory_mb or disk_gb",
            ));
        }
        let id = self.resolve(space)?;
        let SpaceId::Local { name } = &id else {
            return Err(Error::WrongProvider {
                op: "Resizing".into(),
                provider: format!(
                    "{} (only a VM on this computer; resize it on the computer that runs it)",
                    id.provider()
                ),
            });
        };
        self.drop_connection(&id).await;
        let shape = cua_sandbox_core::Resize {
            cpus,
            memory_mb,
            disk_gb,
            probes: Vec::new(),
        };
        self.inner.sandboxes.resize(name, &shape).await?;
        #[cfg(feature = "mcp")]
        self.notify_connected(&id);
        // It booted again: a Space that was on the relay goes back on it.
        #[cfg(feature = "spaces-agents")]
        self.reattach_relay_after_boot(&id).await;
        let mut what = Vec::new();
        if let Some(c) = cpus {
            what.push(format!("{c} vCPUs"));
        }
        if let Some(m) = memory_mb {
            what.push(format!("{m} MiB of memory"));
        }
        if let Some(d) = disk_gb {
            what.push(format!("a {d} GiB disk"));
        }
        Ok(SpaceResize {
            space: id.to_string(),
            cpus,
            memory_mb,
            disk_gb,
            message: format!(
                "{} now has {}; it booted again from its disk.",
                id,
                what.join(", ")
            ),
        })
    }

    pub(crate) async fn set_power(&self, space: &str, on: bool) -> Result<SpacePower> {
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
                    // What extensions keep for a connected Space (its Cua
                    // Volume) ended with the connection above: start it
                    // again now, not only when something next connects.
                    #[cfg(feature = "mcp")]
                    self.notify_connected(&id);
                    // A guest that booted again forgot its relay attachment.
                    #[cfg(feature = "spaces-agents")]
                    self.reattach_relay_after_boot(&id).await;
                    cua_sandbox_core::PowerState::Running
                } else {
                    sandboxes.power_off(name).await?
                };
                let control = sandboxes
                    .power_control(name)
                    .unwrap_or(cua_sandbox_core::PowerControl::Stop);
                #[cfg(feature = "spaces-agents")]
                crate::share::PUBLISH_CHANGED.notify_waiters();
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
                    None => Err(refuse(
                        "relay (only a Space one of your computers provides)",
                    )),
                }
            }
            // Cua Cloud has ended: there is no cloud Space to turn on or off.
            SpaceId::Cloud { .. } => Err(cua_sandbox_core::Error::CloudEnded.into()),
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
    /// Recovery failed (why); a local attempt and its journal are retained.
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

/// Whether `record` remembers cua-spacesd answering: its image runs one.
fn record_runs_spacesd(record: Option<&dpb::Space>) -> bool {
    record.is_some_and(|r| !r.spacesd_version.is_empty() || !r.features.is_empty())
}

/// How long a new local Space's cua-spacesd has to accept the connection
/// once its sandbox is ready. A chosen `timeout` is counted from `clock`
/// (when the image was present), with at least 10 s left; the default gives
/// the handshake [`cua_sandbox_core::SPACESD_READY_TIMEOUT`] however long
/// the boot took.
fn connect_budget(timeout: Option<Duration>, clock: std::time::Instant) -> Duration {
    match timeout {
        Some(t) => t
            .saturating_sub(clock.elapsed())
            .max(Duration::from_secs(10)),
        None => cua_sandbox_core::SPACESD_READY_TIMEOUT,
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
        Some(image) => cua_image::canonical::is_canonical(strip_runtime_prefix(image)),
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

/// The error a thumbnail request gets while the Space's capture backs off
/// (and nothing is cached).
fn thumbnail_failure(f: &crate::thumbnails::CaptureFailure) -> Error {
    let details = cua_spacesd_client::ErrorDetails {
        code: if f.denied { 7 } else { 14 },
        message: format!("{} (not retried for a while)", f.message),
        metadata: Default::default(),
    };
    if f.denied {
        Error::Env(cua_spacesd_client::Error::PermissionDenied(details))
    } else {
        Error::Env(cua_spacesd_client::Error::Transport(details.message))
    }
}

/// Whether a create may overflow from this machine into the linked clouds:
/// no location given and none configured (an explicit `local`, `auto`, a
/// host or a cloud keeps its meaning), not a reuse, no mounts of this
/// computer. The budget, the setting and the linked clouds are checked
/// after ([`Spaces::overflow_allowed`]).
pub(crate) fn may_overflow(
    omitted: bool,
    on: &On,
    source: &cua_sandbox_core::settings::Source,
    reuse: bool,
    mounts: bool,
) -> bool {
    omitted
        && *on == On::Local
        && *source == cua_sandbox_core::settings::Source::Default
        && !reuse
        && !mounts
}

/// Whether a local create failed because this machine has no room (what
/// overflows into the linked clouds).
pub(crate) fn overflows(e: &Error) -> bool {
    matches!(e.tag(), "host_capacity" | "insufficient_disk")
}

/// How long a ready Space waits for spacesd's desktop health to be
/// SERVING (its desktop component can take a moment after the port opens).
const SERVING_WAIT: Duration = Duration::from_secs(10);

/// Waits (bounded by `SERVING_WAIT` and the create's `deadline`) until the
/// Space's spacesd reports desktop component SERVING, so a caller that acts at once
/// (a screenshot) does not hit a half-started desktop. Never an error: a
/// spacesd that does not report health, or stays NOT_SERVING, is still
/// returned as the create's result, as before.
async fn wait_serving(space: &Space, deadline: tokio::time::Instant) {
    let Ok(client) = space.spacesd() else {
        return;
    };
    let until = deadline.min(tokio::time::Instant::now() + SERVING_WAIT);
    loop {
        match client.health().await {
            Ok(h) => {
                // Only the desktop component is waited for: an image
                // without a desktop has none and is ready at once.
                let desktop_starting = h.components.iter().any(|c| {
                    c.name == cua_spacesd_client::DESKTOP_COMPONENT
                        && c.status != cua_spacesd_client::pb::HealthStatus::Serving as i32
                });
                if !desktop_starting {
                    return;
                }
            }
            Err(_) => return,
        }
        if tokio::time::Instant::now() >= until {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// A Mac cannot create a Windows VM locally yet. Refuses a local create up
/// front, in words a person can act on. Remote host placement is checked by
/// the selected host instead.
pub(crate) const WINDOWS_COMING_SOON: &str = "Windows Spaces are coming soon on Mac.";

fn refuse_windows_on_mac(image: Option<&str>, host_is_mac: bool) -> Result<()> {
    let windows = image.is_some_and(|i| {
        cua_image::catalog::catalog_os(i).as_deref() == Some("windows")
            || i.rsplit('/')
                .next()
                .is_some_and(|n| n.to_ascii_lowercase().starts_with("windows"))
    });
    if host_is_mac && windows {
        return Err(Error::invalid(WINDOWS_COMING_SOON));
    }
    Ok(())
}

/// Passes `result` through; on an error runs `teardown` (best effort),
/// logging its failure without replacing the original error.
pub(crate) async fn teardown_on_error<T, E, D>(
    result: Result<T>,
    what: &str,
    name: &str,
    teardown: D,
) -> Result<T>
where
    E: std::fmt::Display,
    D: std::future::Future<Output = std::result::Result<(), E>>,
{
    if result.is_err()
        && let Err(e) = teardown.await
    {
        tracing::warn!(error = %e, %what, %name, "teardown after failed registration also failed");
    }
    result
}

/// The kind a local Space's runtime makes it: the runtime that runs it says
/// what it is (a microVM's guest reports itself as a container, and its image
/// is a root filesystem); else the image's variant; else the kind an external
/// provider presents as.
fn kind_of_runtime(
    runtime: &str,
    variant: &str,
    placed: cua_sandbox_core::placement::Kind,
) -> String {
    match runtime {
        "vz" | "firecracker" | "containerization" => return "microvm".into(),
        "qemu" | "lume" | "qemu-docker" => return "vm".into(),
        _ => {}
    }
    match variant {
        "rootfs" => "container",
        "containerdisk" | "lume" => "vm",
        _ if placed != cua_sandbox_core::placement::Kind::Auto => placed.as_str(),
        _ => "",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    /// Set up turning on This Mac's desktop restarts its cua-spacesd, which
    /// joins the relay again: a cached connection from before is dialed
    /// again (CUA-3974).
    #[test]
    fn a_computer_that_connected_again_is_a_new_session() {
        use super::session_changed;
        assert!(session_changed(
            Some((true, Some(100))),
            Some((true, Some(160)))
        ));
        assert!(!session_changed(
            Some((true, Some(100))),
            Some((true, Some(100)))
        ));
        // Offline now, or never listed: nothing says the connection died.
        assert!(!session_changed(
            Some((true, Some(100))),
            Some((false, None))
        ));
        assert!(!session_changed(None, Some((true, Some(160)))));
        assert!(!session_changed(
            Some((true, None)),
            Some((true, Some(160)))
        ));
        assert!(!session_changed(Some((true, Some(100))), None));
    }

    #[test]
    fn a_spaces_kind_is_what_its_runtime_makes_it() {
        use cua_sandbox_core::placement::Kind;
        // A microVM runs a root filesystem and its guest says "container".
        assert_eq!(kind_of_runtime("vz", "rootfs", Kind::Auto), "microvm");
        assert_eq!(
            kind_of_runtime("firecracker", "rootfs", Kind::Auto),
            "microvm"
        );
        assert_eq!(kind_of_runtime("qemu", "containerdisk", Kind::Auto), "vm");
        assert_eq!(
            kind_of_runtime("container", "rootfs", Kind::Auto),
            "container"
        );
        assert_eq!(
            kind_of_runtime("managed", "rootfs", Kind::Auto),
            "container"
        );
        assert_eq!(kind_of_runtime("other", "", Kind::Auto), "");
    }

    #[tokio::test]
    async fn a_failed_registration_tears_the_fork_down_and_keeps_its_error() {
        use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
        let deleted = AtomicU32::new(0);
        let td = |fail: bool| {
            let deleted = &deleted;
            async move {
                deleted.fetch_add(1, SeqCst);
                if fail { Err("delete refused") } else { Ok(()) }
            }
        };
        let ok = super::teardown_on_error(Ok(7), "fork", "f", td(false)).await;
        assert_eq!(ok.unwrap(), 7);
        assert_eq!(deleted.load(SeqCst), 0, "a registered fork stays");
        for fail in [false, true] {
            let err = super::teardown_on_error::<(), _, _>(
                Err(Error::invalid("register failed")),
                "fork",
                "f",
                td(fail),
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains("register failed"), "{err}");
        }
        assert_eq!(deleted.load(SeqCst), 2);
    }

    #[test]
    fn only_an_unplaced_create_overflows_into_your_clouds() {
        use cua_sandbox_core::settings::Source;
        let default = Source::Default;
        // No location given, none configured: may overflow (when a budget
        // and a linked cloud say so, checked after).
        assert!(may_overflow(true, &On::Local, &default, false, false));
        // An explicit local never overflows; nor auto, a host or a cloud.
        assert!(!may_overflow(
            false,
            &On::Local,
            &Source::Explicit,
            false,
            false
        ));
        assert!(!may_overflow(
            false,
            &On::Auto,
            &Source::Explicit,
            false,
            false
        ));
        assert!(!may_overflow(
            false,
            &On::Provider("aws".into()),
            &Source::Explicit,
            false,
            false
        ));
        // A configured default (`default.on`, CUA_DEFAULT_ON) is the person's choice.
        assert!(!may_overflow(
            true,
            &On::Local,
            &Source::Env("CUA_DEFAULT_ON"),
            false,
            false
        ));
        assert!(!may_overflow(
            true,
            &On::Local,
            &Source::Config("c".into()),
            false,
            false
        ));
        // Reuse and this computer's mounts stay here.
        assert!(!may_overflow(true, &On::Local, &default, true, false));
        assert!(!may_overflow(true, &On::Local, &default, false, true));
        // Only "no room here" overflows; other failures are the answer.
        assert!(overflows(&Error::Sandbox(
            cua_sandbox_core::Error::Runtime(cua_sandbox_core::RuntimeError::HostCapacity(
                "full".into()
            ))
        )));
        assert!(!overflows(&Error::invalid("bad image")));
        assert!(!overflows(&Error::LimitExceeded("cap".into())));
    }

    use super::*;

    #[test]
    fn a_windows_create_from_a_mac_is_refused_in_plain_words() {
        for image in ["windows", "windows:2022", "ghcr.io/trycua/windows:2022"] {
            let e = refuse_windows_on_mac(Some(image), true).unwrap_err();
            assert!(e.to_string().contains("coming soon on Mac"), "{image}: {e}");
            assert!(refuse_windows_on_mac(Some(image), false).is_ok());
        }
        assert!(refuse_windows_on_mac(Some("ghcr.io/trycua/linux:24.04"), true).is_ok());
        assert!(refuse_windows_on_mac(None, true).is_ok());
    }

    #[test]
    fn the_default_connect_budget_does_not_depend_on_the_boot() {
        let now = std::time::Instant::now();
        assert_eq!(
            connect_budget(None, now),
            cua_sandbox_core::SPACESD_READY_TIMEOUT
        );
        if let Some(long_ago) = now.checked_sub(Duration::from_secs(900)) {
            assert_eq!(
                connect_budget(None, long_ago),
                cua_sandbox_core::SPACESD_READY_TIMEOUT
            );
        }
    }

    #[test]
    fn a_chosen_connect_budget_counts_from_the_image_being_present() {
        let t = Duration::from_secs(600);
        let left = connect_budget(Some(t), std::time::Instant::now());
        assert!(left > Duration::from_secs(590), "{left:?}");
        if let Some(spent) = std::time::Instant::now().checked_sub(Duration::from_secs(900)) {
            assert_eq!(connect_budget(Some(t), spent), Duration::from_secs(10));
        }
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
    fn a_relay_machine_lists_the_os_its_spacesd_reported() {
        let spaces = Spaces::builder()
            .home(tempfile::tempdir().unwrap().keep())
            .build();
        let machine: crate::relay::RelayMachine = serde_json::from_value(serde_json::json!({
            "id": "96fedb7e1be65c3d31fa18587febde2c",
            "name": "gamma-4 Mac Studio",
            "online": true,
        }))
        .unwrap();
        *spaces.inner.relay_cache.lock().unwrap() = vec![machine];
        let id = "relay:96fedb7e1be65c3d31fa18587febde2c";
        let listed = |s: &Spaces| s.list().unwrap().into_iter().find(|i| i.id == id).unwrap();
        // Before any connect the listing says nothing about the OS.
        assert_eq!(listed(&spaces).os, "");
        spaces.inner.relay_reported.lock().unwrap().insert(
            id.into(),
            ReportedFacts {
                os: "macos".into(),
                os_name: "macOS".into(),
                os_pretty_name: "macOS Tahoe 26.0".into(),
                arch: "arm64".into(),
            },
        );
        let info = listed(&spaces);
        assert_eq!(
            (
                info.os.as_str(),
                info.os_pretty_name.as_str(),
                info.arch.as_str()
            ),
            ("macos", "macOS Tahoe 26.0", "arm64")
        );
        assert_eq!(
            spaces.info(&SpaceId::parse(id).unwrap()).unwrap().os,
            "macos"
        );
        // Offline, the listing says nothing it learned at an earlier connect.
        spaces.inner.relay_cache.lock().unwrap()[0].online = false;
        assert_eq!(listed(&spaces).os, "");
        assert_eq!(spaces.info(&SpaceId::parse(id).unwrap()).unwrap().os, "");
    }

    #[test]
    fn a_relay_host_lists_the_os_it_registered_with() {
        let spaces = Spaces::builder()
            .home(tempfile::tempdir().unwrap().keep())
            .build();
        let machine: crate::relay::RelayMachine = serde_json::from_value(serde_json::json!({
            "id": "96fedb7e1be65c3d31fa18587febde2c",
            "name": "gamma-4 Mac Studio",
            "online": true,
            "meta": {"cua.host.spaces": "on", "cua.host.os": "macos", "cua.host.arch": "arm64"},
        }))
        .unwrap();
        *spaces.inner.relay_cache.lock().unwrap() = vec![machine];
        let info = spaces.list().unwrap().remove(0);
        assert_eq!((info.os.as_str(), info.arch.as_str()), ("macos", "arm64"));
    }

    #[test]
    fn what_a_relay_machine_reported_outlives_the_process() {
        let home = tempfile::tempdir().unwrap().keep();
        let id = "relay:96fedb7e1be65c3d31fa18587febde2c";
        save_reported(
            &home,
            &HashMap::from([(
                id.to_string(),
                ReportedFacts {
                    os: "macos".into(),
                    os_name: "macOS".into(),
                    os_pretty_name: "macOS Tahoe 26.0".into(),
                    arch: "arm64".into(),
                },
            )]),
        )
        .unwrap();
        // A later process (`cua spaces ls`) that has connected to nothing.
        let spaces = Spaces::builder().home(home).build();
        let machine: crate::relay::RelayMachine = serde_json::from_value(serde_json::json!({
            "id": "96fedb7e1be65c3d31fa18587febde2c",
            "name": "gamma-4 Mac Studio",
            "online": true,
        }))
        .unwrap();
        *spaces.inner.relay_cache.lock().unwrap() = vec![machine];
        let info = spaces.list().unwrap().remove(0);
        assert_eq!(
            (
                info.os.as_str(),
                info.os_pretty_name.as_str(),
                info.arch.as_str()
            ),
            ("macos", "macOS Tahoe 26.0", "arm64")
        );
    }

    #[test]
    fn labels_are_dns_safe() {
        assert_eq!(sanitize_label("Client_ID.42"), "client-id-42");
        assert_eq!(sanitize_label("--x--"), "x");
        assert_eq!(sanitize_label(&"a".repeat(80)).len(), 63);
    }

    #[test]
    fn generated_names_are_the_sdks_own_form() {
        let name = cua_spaces_contract::names::pet_name(|_| false);
        assert!(is_generated_name(&name), "{name}");
        assert!(is_generated_name(&format!("space-{}", random_suffix())));
        assert!(is_generated_name("space-000000002a"));
        for other in [
            "space-2a",
            "space-000000002A",
            "dev",
            "space-000000002ag",
            "my-space-000000002a",
        ] {
            assert!(!is_generated_name(other), "{other}");
        }
    }
}
