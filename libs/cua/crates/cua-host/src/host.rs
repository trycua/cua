//! [`Host`]: setup / status / stop / start / remove.

use crate::driver::{self, DriverSource};
use crate::helper_migration::{
    self, LegacyService, MigrationFacts, MigrationPlan, MigrationRecord, ServiceMigration,
    VerifyTiming,
};
use crate::preflight::{self, SessionProbe};
use crate::relay::{AccountTokens, ConnectedClient, MachinePatch, RegisterRequest, RelayClient};
use crate::service::{self, RunnerKind, ServiceManager, ServiceSpec, ServiceState};
use crate::{Error, Result};
use cua_home::channel::{CUA_CHANNEL_VAR, CuaChannel};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where a host setup stopped (see [`Host::setup_staged`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupStage {
    /// Checking the request and this machine (GUI session, address).
    Preflight,
    /// Getting the account's token (relay mode).
    Token,
    /// Registering with the relay.
    Register,
    /// Getting cua-spacesd (download or local copy).
    Download,
    /// Installing and starting the host service.
    Service,
}

impl SetupStage {
    /// The stage as usage data and logs spell it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::Token => "token",
            Self::Register => "register",
            Self::Download => "download",
            Self::Service => "service",
        }
    }
}

/// How clients reach this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostMode {
    /// Join a cua-relay (outbound WSS; the default is
    /// [`crate::DEFAULT_RELAY_URL`]).
    Relay {
        /// Relay base URL.
        url: String,
    },
    /// Serve on `listen` with a generated env token.
    Direct {
        /// Listen address (`0.0.0.0:3211` for every interface).
        listen: SocketAddr,
    },
}

/// The file under `<home>/host` that keeps this computer's registrations
/// under other accounts than the signed-in one.
const REGISTRATIONS_FILE: &str = "registrations.json";

/// A relay registration of this computer under one account.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredRegistration {
    machine_id: String,
    machine_token: String,
}

/// Account id -> the registration of this computer under it, for accounts
/// that signed in here before and are not signed in now.
type Registrations = std::collections::BTreeMap<String, StoredRegistration>;

/// Options for [`Host::setup`].
#[derive(Clone, Debug)]
pub struct SetupOptions {
    /// Relay or direct.
    pub mode: HostMode,
    /// Display name (default: the host name).
    pub name: Option<String>,
    /// Accounts (ids or emails) allowed besides the owner (relay mode).
    pub allow: Vec<String>,
    /// The driver binary; else `CUA_SPACESD_BIN`, a bundled one, or the
    /// release download.
    pub driver_bin: Option<PathBuf>,
    /// How to run it.
    pub runner: RunnerKind,
    /// Expose this machine's own desktop as a Space (default on; see
    /// [`crate::HostProfile`]).
    pub share_desktop: bool,
    /// Accept Space create, list and delete requests and run them on this
    /// machine's runtimes (default off): from the owner's enrolled devices
    /// in relay mode, from the env token holder in direct mode.
    pub provide_spaces: bool,
    /// Provided Spaces at once (`None`: [`crate::provided::DEFAULT_MAX_SPACES`]).
    pub max_spaces: Option<u32>,
    /// The `cua` CLI the driver starts the host's cua daemon with when it
    /// is not running (default: this executable when it is `cua`, else
    /// `CUA_BIN`, else `cua` on `PATH`).
    pub cua_bin: Option<PathBuf>,
    /// Direct mode with Spaces: accept host calls (and connections to the
    /// Spaces this machine forwards) from any address, not only loopback,
    /// Tailscale and private LAN addresses, and allow a public listen
    /// address (see [`crate::direct`]). Default off.
    pub allow_any_address: bool,
    /// Direct mode: also join this relay in static-token mode (a
    /// self-hosted `cua-relay --tokens ...`), so callers that cannot reach
    /// the direct address reach this machine at `<relay>/m/<machine id>`
    /// with its env token. No account is involved.
    pub static_relay: Option<StaticRelay>,
    /// Direct mode on your own tailnet (Tailscale or Headscale): callers
    /// are identified by their tailnet login (the owner is this machine's
    /// tailnet user; `allow` names other tailnet users), so they need no
    /// env token. See [`crate::direct::tailnet_login`].
    pub tailnet: bool,
    /// Install the driver before registering (a single-use pairing code is
    /// then spent only once the download worked). Default off.
    pub download_first: bool,
    /// The machine exists only for Cua work (a cloud instance made for it,
    /// such as a `cua cloud add aws` host): it offers its own Space to jobs
    /// (`kind = "machine"`), where they run with its GPU and drivers as
    /// they are. Never for a computer you also use; default off.
    pub dedicated: bool,
}

/// A relay in static-token mode ([`SetupOptions::static_relay`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticRelay {
    /// Relay base URL.
    pub url: String,
    /// One of its machine-registration tokens.
    pub token: String,
}

impl SetupOptions {
    /// Relay mode at `url` with defaults.
    pub fn relay(url: impl Into<String>) -> Self {
        Self {
            mode: HostMode::Relay { url: url.into() },
            name: None,
            allow: vec![],
            driver_bin: None,
            runner: RunnerKind::Auto,
            share_desktop: true,
            provide_spaces: false,
            max_spaces: None,
            cua_bin: None,
            allow_any_address: false,
            static_relay: None,
            tailnet: false,
            download_first: false,
            dedicated: false,
        }
    }

    /// Applies a profile's two settings.
    pub fn profile(mut self, profile: crate::HostProfile) -> Self {
        (self.share_desktop, self.provide_spaces) = profile.settings();
        self
    }

    /// Direct mode on `listen` with defaults.
    pub fn direct(listen: SocketAddr) -> Self {
        Self {
            mode: HostMode::Direct { listen },
            name: None,
            allow: vec![],
            driver_bin: None,
            runner: RunnerKind::Auto,
            share_desktop: true,
            provide_spaces: false,
            max_spaces: None,
            cua_bin: None,
            allow_any_address: false,
            static_relay: None,
            tailnet: false,
            download_first: false,
            dedicated: false,
        }
    }
}

/// `<home>/host/host.json`: the policy `cua-spacesd join --host-policy`
/// enforces on relay-asserted identities (reloaded when it changes).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPolicy {
    /// Owner account id.
    pub owner: String,
    /// Owner email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    /// Allowed account ids / emails besides the owner.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Account ids / emails that may only watch (presence and a view-only
    /// stream); an entry here wins over `allow`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub viewers: Vec<String>,
    /// Also accept accounts the relay says the machine is shared with.
    #[serde(default = "yes")]
    pub trust_relay_allowlist: bool,
    /// Accepting clients.
    #[serde(default = "yes")]
    pub sharing: bool,
    /// This machine's own desktop is a Space. Off: relayed callers reach
    /// only `HostSpacesService` and the capabilities probe, and the driver
    /// runs without its desktop services.
    #[serde(default = "yes")]
    pub share_desktop: bool,
    /// Accept Space create, list and delete requests (`HostSpacesService`).
    #[serde(default)]
    pub provide_spaces: bool,
    /// Provided Spaces at once (0: no limit).
    #[serde(default = "default_max_spaces")]
    pub max_spaces: u32,
    /// macOS VMs at once on this Mac (at most two, Apple's license).
    #[serde(default = "default_max_macos")]
    pub max_macos_vms: u32,
    /// Where the driver reaches (and how it starts) this machine's cua
    /// daemon, which creates the provided Spaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spaces_daemon: Option<SpacesDaemon>,
    /// Direct mode: Spaces are provided without the relay, each reached at
    /// this machine's address on a forwarded port. `None` in relay mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct: Option<DirectHosting>,
    /// Identify tailnet callers by their tailnet login (`--tailnet`): the
    /// owner email and `allow` / `viewers` then name tailnet users.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tailnet_identity: bool,
    /// The machine exists only for Cua work (`--dedicated`, which `cua cloud add
    /// aws` hosts are set up with): jobs may run in its own Space
    /// (`kind = "machine"`). Off by default, and never on a computer you
    /// also use.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dedicated: bool,
    /// Forward `/share/...` requests to the cua daemon's share gateway
    /// (`cua share`): links to ports, files and folders the owner shares,
    /// reached through the relay. Off until the first `cua share` (or a
    /// relay-mode setup) turns it on; it takes effect on the next request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shares: bool,
    /// The daemon's share gateway socket (`<cua home>/share/gateway.sock`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shares_gateway: Option<String>,
}

/// How a host in direct mode provides Spaces (see [`crate::direct`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectHosting {
    /// The direct listener (`ip:port`); each Space's forwarded port is
    /// bound on the same IP.
    pub listen: String,
    /// Accept host calls and forwarded connections from any address, not
    /// only loopback, Tailscale and private LAN addresses.
    #[serde(default)]
    pub allow_any_address: bool,
}

/// How the host's driver reaches this machine's cua daemon.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpacesDaemon {
    /// The daemon's Unix socket (`<cua home>/cua.sock`).
    pub socket: String,
    /// The cua home the daemon runs with.
    pub cua_home: String,
    /// The `cua` CLI that starts it (`cua daemon start`) when the socket
    /// does not answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cua_bin: Option<String>,
}

impl HostPolicy {
    /// The Spaces settings this policy holds.
    pub fn spaces_settings(&self) -> crate::HostSpacesSettings {
        crate::HostSpacesSettings {
            share_desktop: self.share_desktop,
            provide_spaces: self.provide_spaces,
            max_spaces: self.max_spaces,
            max_macos_vms: self
                .max_macos_vms
                .min(crate::provided::MACOS_VM_LICENSE_LIMIT),
        }
    }
}

fn yes() -> bool {
    true
}

fn default_max_spaces() -> u32 {
    crate::provided::DEFAULT_MAX_SPACES
}

fn default_max_macos() -> u32 {
    crate::provided::MACOS_VM_LICENSE_LIMIT
}

/// The `cua` CLI to start the host's daemon with: `explicit`, else the
/// `cua` of the app bundle this process runs in (an app's daemon is its own
/// build), else this executable when it is `cua`, else `CUA_BIN`, else `cua`
/// on `PATH`.
fn find_cua_bin(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    if let Some(p) = cua_home::bundled_cua() {
        return Some(p);
    }
    if let Ok(me) = std::env::current_exe()
        && me.file_stem().is_some_and(|s| s == "cua")
    {
        return Some(me);
    }
    if let Some(p) = std::env::var_os("CUA_BIN").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let exe = if cfg!(windows) { "cua.exe" } else { "cua" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(exe))
        .find(|p| p.is_file())
}

/// `<home>/host/config.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostConfig {
    /// `relay` or `direct`.
    pub mode: String,
    /// Relay base URL (relay mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    /// Listen address (direct mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<String>,
    /// Display name.
    pub name: String,
    /// Machine id (relay mode, and direct mode with a static relay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    /// Direct mode: the static-token relay this machine also joins
    /// ([`SetupOptions::static_relay`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub static_relay: Option<String>,
    /// Runner (`systemd`, `launchd`, `windows-task`, `process`).
    pub runner: String,
    /// Installed driver binary.
    pub driver_bin: PathBuf,
    /// Sharing (direct mode: the service runs).
    #[serde(default = "yes")]
    pub sharing: bool,
    /// This machine's desktop is a Space.
    #[serde(default = "yes")]
    pub share_desktop: bool,
    /// This machine provides Spaces (relay mode through the relay, direct
    /// mode on its direct address).
    #[serde(default)]
    pub provide_spaces: bool,
    /// Relay sharing is paused because nobody is signed in to the owner's
    /// account on this machine (see [`Host::pause_signed_out`]): the
    /// service is uninstalled so it neither runs nor starts at login, and
    /// the setup (machine token, policy, this file) stays so
    /// [`Host::resume_signed_in`] can bring it back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused_signed_out: bool,
    /// Sharing stopped because both settings were turned off (nothing to
    /// share): turning either on again shares again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped_for_nothing: bool,
    /// Direct mode on the owner's tailnet with tailnet identity
    /// (`--tailnet`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tailnet: bool,
    /// `CUA_MESH` as it was set where `cua host setup` ran (an explicit
    /// opt-in to the WireGuard®-based mesh, or out of it): passed on to the
    /// service. Unset, the service follows its own default (prefer).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mesh: Option<String>,
    /// `CUA_HOST_USD_PER_HOUR` as it was set where `cua host setup` ran:
    /// what this machine costs per hour (a cloud host's on-demand price),
    /// passed on to the service, which shows it to the pool. Unset: an owned
    /// machine ($0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_per_hour: Option<String>,
}

/// `CUA_MESH` where setup runs, when set.
fn mesh_env() -> Option<String> {
    std::env::var("CUA_MESH")
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

/// A macOS privacy pane the user must grant the driver in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionHint {
    /// `screen-recording` or `accessibility`.
    pub id: String,
    /// Human title.
    pub title: String,
    /// `x-apple.systempreferences:` URL of the pane (for the user to open).
    pub settings_url: String,
    /// What to do there.
    pub instructions: String,
}

/// The permission panes for `os` (`std::env::consts::OS` values). Only
/// macOS needs any. Never opened or granted by this crate.
pub fn permission_hints(os: &str, driver: &Path) -> Vec<PermissionHint> {
    if os != "macos" {
        return vec![];
    }
    let d = service_name(driver);
    // The app's executable runs the service: both are "Cua Spaces".
    let screen = d.clone();
    vec![
        PermissionHint {
            id: "screen-recording".into(),
            title: "Screen Recording".into(),
            settings_url:
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
                    .into(),
            instructions: format!(
                "System Settings → Privacy & Security → Screen & System Audio Recording: turn on {screen}, so people you share with can see this screen."
            ),
        },
        PermissionHint {
            id: "accessibility".into(),
            title: "Accessibility".into(),
            settings_url:
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
                    .into(),
            instructions: format!(
                "System Settings → Privacy & Security → Accessibility: turn on {d}, so they can control the mouse and keyboard."
            ),
        },
    ]
}

/// How System Settings lists the host service: "Cua Spaces" when the app
/// runs it, else "cua-spacesd (<path>)".
fn service_name(driver: &Path) -> String {
    if crate::service::is_app_helper_launcher(driver) {
        crate::service::app_helper_name().to_string()
    } else {
        format!("cua-spacesd ({})", driver.display())
    }
}

/// What a Mac that provides Spaces must allow in Local Network privacy, or
/// `None` (another OS, or no Spaces provided). Its macOS Spaces are Lume
/// VMs on vmnet (192.168.64.x), and the cua daemon that creates them must
/// reach the guest; macOS blocks that ("No route to host") until Local
/// Network access is allowed for the process responsible for the daemon.
/// For a host set up with the CLI, that is the cua-spacesd LaunchAgent
/// (`driver`), which starts the daemon when a Space is created, unless the
/// daemon was already running from Terminal or an SSH session: then it is
/// that app's (and over SSH nobody can be asked). macOS asks once, on this
/// Mac's own screen; it can't be granted from here. Linux Spaces (reached
/// through localhost) don't need it.
pub fn local_network_hint(os: &str, provide_spaces: bool, driver: &Path) -> Option<PermissionHint> {
    if os != "macos" || !provide_spaces {
        return None;
    }
    let d = service_name(driver);
    let short = if crate::service::is_app_helper_launcher(driver) {
        crate::service::app_helper_name()
    } else {
        "cua-spacesd"
    };
    Some(PermissionHint {
        id: "local-network".into(),
        title: "Local Network".into(),
        settings_url:
            "x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork".into(),
        instructions: format!(
            "macOS Spaces on this Mac need Local Network access for {d}. When \
             macOS asks \"Allow {short} to find devices on local networks?\" on this Mac's \
             screen, click Allow; or turn on {short} in System Settings → Privacy & \
             Security → Local Network. If the {cli} daemon was started from Terminal or over SSH, \
             run `{cli} daemon stop` so the host service starts it at the next create (over SSH \
             nothing can grant it). Without it, macOS Space creates fail with \"No route to \
             host\"; Linux Spaces don't need it.",
            cli = crate::cli()
        ),
    })
}

/// The relay machine meta key saying whether this machine provides Spaces
/// (`on` / `off`), as of its last setup. Your other devices list a machine
/// that provides Spaces in their "Run on" menu even while it is offline
/// (with why), instead of leaving it out.
pub const META_PROVIDES_SPACES: &str = "cua.host.spaces";

/// The relay machine meta key with this machine's OS family (`macos`,
/// `linux`, `windows`), as of its last setup. The relay directory says
/// nothing else about the OS, so without it a client lists the machine
/// with no OS until it has connected to it.
pub const META_OS: &str = "cua.host.os";

/// The relay machine meta key with this machine's architecture (`arm64`,
/// `amd64`: the names Spaces use), as of its last setup.
pub const META_ARCH: &str = "cua.host.arch";

/// The relay machine meta key with the cloud a pool host runs in
/// (`hetzner`, `aws`...), from `CUA_HOST_CLOUD` where `cua host setup` ran
/// (the cloud-init of a machine the pool added sets it). Whichever machine
/// coordinates the pool places `--on <cloud>` work on it by this.
pub const META_CLOUD: &str = "cua.host.cloud";

/// `CUA_HOST_CLOUD`, when it names a cloud (lowercase letters and digits).
fn host_cloud() -> Option<String> {
    cloud_word(std::env::var("CUA_HOST_CLOUD").ok())
}

fn cloud_word(v: Option<String>) -> Option<String> {
    let v = v?.trim().to_ascii_lowercase();
    (!v.is_empty() && v.len() <= 32 && v.chars().all(|c| c.is_ascii_alphanumeric())).then_some(v)
}

/// This build's OS family, as Spaces name it.
fn os_family() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        "linux" => "linux",
        _ => "",
    }
}

/// This build's architecture, as Spaces name it.
fn arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        _ => "",
    }
}

/// The meta a host registers with.
fn host_meta(provide_spaces: bool) -> std::collections::BTreeMap<String, String> {
    let mut meta = std::collections::BTreeMap::from([(
        META_PROVIDES_SPACES.to_string(),
        if provide_spaces { "on" } else { "off" }.to_string(),
    )]);
    for (key, value) in [(META_OS, os_family()), (META_ARCH, arch())] {
        if !value.is_empty() {
            meta.insert(key.to_string(), value.to_string());
        }
    }
    if let Some(cloud) = host_cloud() {
        meta.insert(META_CLOUD.to_string(), cloud);
    }
    meta
}

/// What `status` reports (camelCase for the app).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    /// Set up (config present).
    pub configured: bool,
    /// `relay` or `direct`.
    pub mode: Option<String>,
    /// Relay base URL.
    pub relay_url: Option<String>,
    /// `http://<ip>:<port>` clients use in direct mode.
    pub direct_url: Option<String>,
    /// Where the direct-mode env token is (0600); never the token itself.
    pub env_token_path: Option<String>,
    /// Machine id (relay mode).
    pub machine_id: Option<String>,
    /// Display name.
    pub name: Option<String>,
    /// Accepting clients.
    pub sharing: bool,
    /// The service.
    pub service: ServiceState,
    /// Connected to the relay (relay mode, when the relay answered).
    pub online: Option<bool>,
    /// Who is connected (relay mode).
    pub clients: Vec<ConnectedClient>,
    /// Allowlist (relay mode).
    pub allow: Vec<String>,
    /// OS permission panes the user must grant (macOS).
    pub permissions: Vec<PermissionHint>,
    /// Last error talking to the relay, if any.
    pub error: Option<String>,
    /// Who reached this machine recently, newest first (the driver's
    /// hash-chained access log).
    #[serde(default)]
    pub recent_access: Vec<crate::access::AccessRecord>,
    /// Set when the access log does not verify (edited or truncated).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_log_error: Option<String>,
    /// This machine's desktop is a Space (the default).
    #[serde(default = "yes")]
    pub share_desktop: bool,
    /// This machine provides Spaces to the owner's devices.
    #[serde(default)]
    pub provide_spaces: bool,
    /// Provided Spaces at once (0: no limit).
    #[serde(default)]
    pub max_spaces: u32,
    /// macOS VMs at once (at most two).
    #[serde(default)]
    pub max_macos_vms: u32,
    /// The Spaces this machine provides now (without their secrets).
    #[serde(default)]
    pub provided_spaces: Vec<crate::provided::ProvidedSpace>,
    /// Every remote create, delete and refusal, and settings changes,
    /// newest first.
    #[serde(default)]
    pub spaces_audit: Vec<crate::provided::SpacesAuditRecord>,
    /// Set when the Spaces audit does not verify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spaces_audit_error: Option<String>,
    /// Relay sharing is paused until the owner signs in again
    /// ([`Host::pause_signed_out`]).
    #[serde(default)]
    pub paused_signed_out: bool,
    /// The account this machine is registered to (relay mode): its id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    /// The owner's email, when the relay gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    /// macOS: a Cua Spaces app that can run the host service is
    /// installed, so the host service can run as it.
    #[serde(default)]
    pub helper_available: bool,
    /// macOS: the old hand-written LaunchAgent
    /// (`com.trycua.spacesd.host`) is still there (loaded or its plist).
    #[serde(default)]
    pub legacy_service: bool,
}

/// Files under `<home>/host`.
#[derive(Clone, Debug)]
pub struct HostPaths {
    /// `<home>/host`.
    pub dir: PathBuf,
    /// `<home>/spacesd/id`.
    pub machine_id: PathBuf,
    /// `<home>/spacesd` (driver data dir).
    pub driver_data: PathBuf,
}

impl HostPaths {
    /// Paths for cua home `home` (`~/.cua`).
    pub fn new(home: &Path) -> Self {
        Self {
            dir: home.join("host"),
            machine_id: home.join("spacesd").join("id"),
            driver_data: home.join("spacesd"),
        }
    }
    /// `config.json`.
    pub fn config(&self) -> PathBuf {
        self.dir.join("config.json")
    }
    /// `host.json` (driver policy).
    pub fn policy(&self) -> PathBuf {
        self.dir.join("host.json")
    }
    /// `machine-token` (0600).
    pub fn machine_token(&self) -> PathBuf {
        self.dir.join("machine-token")
    }
    /// `env-token` (0600).
    pub fn env_token(&self) -> PathBuf {
        self.dir.join("env-token")
    }
    /// `relay-token` (0600): a static relay's registration token.
    pub fn relay_token(&self) -> PathBuf {
        self.dir.join("relay-token")
    }
    /// `relay-jwks.json`: the relay keys returned at registration.
    pub fn relay_jwks(&self) -> PathBuf {
        self.dir.join("relay-jwks.json")
    }
    /// The installed driver.
    pub fn driver_bin(&self) -> PathBuf {
        self.dir.join("bin").join(driver::binary_name())
    }
    /// `driver.log`.
    pub fn log(&self) -> PathBuf {
        self.dir.join("driver.log")
    }
    /// The driver's access log (`<driver data>/access.log`).
    pub fn access_log(&self) -> PathBuf {
        self.driver_data.join("access.log")
    }
}

pub(crate) fn write_secret(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A fresh, exclusively created 0600 temp file (never an existing file or
    // symlink) renamed over the target, so the target is either the old or
    // the new content and is never readable by others.
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let mut last = None;
    for _ in 0..16 {
        let tmp = dir.join(format!(
            ".{name}.{}.{}.tmp",
            std::process::id(),
            random_hex(8)
        ));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut f = match opts.open(&tmp) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last = Some(e);
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        use std::io::Write as _;
        let written = f.write_all(content.as_bytes()).and_then(|()| f.sync_all());
        drop(f);
        if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, path)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        return Ok(());
    }
    Err(last
        .unwrap_or_else(|| std::io::Error::other("no free temp name"))
        .into())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text =
        serde_json::to_string_pretty(value).map_err(|e| Error::Internal(e.to_string()))? + "\n";
    write_secret(path, &text)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice(&b)
            .map(Some)
            .map_err(|e| Error::Internal(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn read_secret(path: &Path) -> Result<String> {
    let s = std::fs::read_to_string(path)
        .map_err(|e| Error::NotFound(format!("{}: {e}", path.display())))?;
    Ok(s.trim().to_string())
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn random_hex(bytes: usize) -> String {
    let v: Vec<u8> = (0..bytes).map(|_| rand::random::<u8>()).collect();
    hex::encode(v)
}

/// Machine ids are `[a-z0-9-]{8,64}` (the relay's rule).
fn valid_machine_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

/// The environment variables of cua-spacesd's `--port` and `--quic-port`.
const PORT_ENV: &str = "CUA_ENV_PORT";
const QUIC_PORT_ENV: &str = "CUA_ENV_QUIC_PORT";

/// The environment variable with this machine's cost per hour (US
/// dollars), passed on to the host service.
pub const HOST_USD_PER_HOUR: &str = "CUA_HOST_USD_PER_HOUR";

/// [`HOST_USD_PER_HOUR`] where setup runs, when it is a non-negative
/// number.
fn host_usd_per_hour() -> Option<String> {
    let v = std::env::var(HOST_USD_PER_HOUR).ok()?;
    let v = v.trim();
    v.parse::<f64>()
        .ok()
        .filter(|p| p.is_finite() && *p >= 0.0)
        .map(|_| v.to_string())
}

/// Loads `path` or creates a random id there (same format as the driver).
fn load_or_create_machine_id(path: &Path) -> Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let id = existing.trim().to_string();
        if valid_machine_id(&id) {
            return Ok(id);
        }
    }
    let id = random_hex(16);
    write_secret(path, &format!("{id}\n"))?;
    Ok(id)
}

/// This machine's host name.
pub(crate) fn hostname() -> String {
    for var in ["CUA_HOST_NAME", "COMPUTERNAME", "HOSTNAME"] {
        if let Ok(v) = std::env::var(var)
            && !v.trim().is_empty()
        {
            return v.trim().to_string();
        }
    }
    if let Ok(v) = std::fs::read_to_string("/etc/hostname")
        && !v.trim().is_empty()
    {
        return v.trim().to_string();
    }
    std::process::Command::new("hostname")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this computer".into())
}

/// The name a person knows this machine by, for display and as the default
/// relay machine / device name: `CUA_HOST_NAME` if set, then on macOS the
/// Sharing "Computer Name" ("cua's Mac Studio"), else the host name with a
/// trailing `.local` / `.localdomain` dropped ("Mac.localdomain" -> "Mac").
pub(crate) fn friendly_name() -> String {
    if let Ok(v) = std::env::var("CUA_HOST_NAME")
        && !v.trim().is_empty()
    {
        return v.trim().to_string();
    }
    #[cfg(target_os = "macos")]
    if let Some(name) = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
    {
        return name;
    }
    strip_local_suffix(&hostname())
}

/// Drops an mDNS / resolver suffix from a host name; keeps the name when
/// nothing would be left.
fn strip_local_suffix(host: &str) -> String {
    let host = host.trim();
    let lower = host.to_ascii_lowercase();
    for suffix in [".localdomain", ".local"] {
        if lower.ends_with(suffix) && host.len() > suffix.len() {
            return host[..host.len() - suffix.len()].to_string();
        }
    }
    host.to_string()
}

use crate::direct::lan_ip;

fn direct_url(listen: &str) -> Option<String> {
    let addr: SocketAddr = listen.parse().ok()?;
    let ip = if addr.ip().is_unspecified() {
        lan_ip().unwrap_or(addr.ip())
    } else {
        addr.ip()
    };
    Some(match ip {
        std::net::IpAddr::V6(v6) => format!("http://[{v6}]:{}", addr.port()),
        v4 => format!("http://{v4}:{}", addr.port()),
    })
}

fn user_home(cua_home: &Path) -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .or_else(|| cua_home.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// A change to this machine's Spaces settings ([`Host::configure`]);
/// `None` keeps the current value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostSettingsChange {
    /// Share this desktop.
    pub share_desktop: Option<bool>,
    /// Provide Spaces.
    pub provide_spaces: Option<bool>,
    /// Provided Spaces at once (0: no limit).
    pub max_spaces: Option<u32>,
    /// macOS VMs at once (0 to 2).
    pub max_macos_vms: Option<u32>,
    /// The `cua` CLI that starts the daemon.
    pub cua_bin: Option<PathBuf>,
}

fn settings_detail(desktop: bool, provide: bool) -> String {
    format!(
        "share_desktop={} provide_spaces={}",
        if desktop { "on" } else { "off" },
        if provide { "on" } else { "off" }
    )
}

/// Host setup and control for one cua home.
pub struct Host {
    home: PathBuf,
    paths: HostPaths,
    manager: Option<Arc<dyn ServiceManager>>,
    preflight: Option<Arc<dyn SessionProbe>>,
    /// The Cua Spaces app with the helper (tests; else found).
    helper_app: Option<PathBuf>,
    /// The helper's runner (tests; else [`service::AppHelper`]).
    helper: Option<Arc<dyn ServiceManager>>,
    /// The old LaunchAgent (tests; else [`service::Launchd`]).
    legacy: Option<Arc<dyn LegacyService>>,
    verify: VerifyTiming,
}

impl std::fmt::Debug for Host {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Host").field("home", &self.home).finish()
    }
}

impl Host {
    /// Host state under `home` (the cua home, `~/.cua`).
    pub fn new(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        Self {
            paths: HostPaths::new(&home),
            home,
            manager: None,
            preflight: None,
            helper_app: None,
            helper: None,
            legacy: None,
            verify: VerifyTiming::default(),
        }
    }

    /// Uses `app` as the installed Cua Spaces app with its helper, run by
    /// `helper`, and `legacy` as the old LaunchAgent (tests).
    pub fn with_app_helper(
        mut self,
        app: PathBuf,
        helper: Arc<dyn ServiceManager>,
        legacy: Arc<dyn LegacyService>,
    ) -> Self {
        self.helper_app = Some(app);
        self.helper = Some(helper);
        self.legacy = Some(legacy);
        self
    }

    /// How long a moved host service has to prove itself (tests).
    pub fn with_verify_timing(mut self, timing: VerifyTiming) -> Self {
        self.verify = timing;
        self
    }

    /// The installed Cua Spaces app with its helper.
    fn helper_app(&self) -> Option<PathBuf> {
        if self.helper.is_some() {
            return self.helper_app.clone();
        }
        service::find_app_helper()
    }

    /// The helper's runner for `app`.
    fn helper_manager(&self, app: &Path) -> Arc<dyn ServiceManager> {
        match &self.helper {
            Some(h) => h.clone(),
            None => Arc::new(service::AppHelper::for_current_user(
                app.to_path_buf(),
                &user_home(&self.home),
            )),
        }
    }

    /// The old LaunchAgent.
    fn legacy(&self) -> Arc<dyn LegacyService> {
        match &self.legacy {
            Some(l) => l.clone(),
            None => Arc::new(service::Launchd::for_current_user(&user_home(&self.home))),
        }
    }

    /// Uses `manager` instead of the OS runner (tests, embedding).
    pub fn with_service_manager(mut self, manager: Arc<dyn ServiceManager>) -> Self {
        self.manager = Some(manager);
        self
    }

    /// Uses `probe` instead of the real macOS preflight checks (tests).
    pub fn with_preflight_probe(mut self, probe: Arc<dyn SessionProbe>) -> Self {
        self.preflight = Some(probe);
        self
    }

    /// The preflight probe: `with_preflight_probe`'s, else the real one.
    fn preflight_probe(&self) -> Arc<dyn SessionProbe> {
        self.preflight
            .clone()
            .unwrap_or_else(|| Arc::new(preflight::SystemProbe))
    }

    /// File locations.
    pub fn paths(&self) -> &HostPaths {
        &self.paths
    }

    /// The stored configuration, if set up.
    pub fn config(&self) -> Result<Option<HostConfig>> {
        read_json(&self.paths.config())
    }

    /// The stored driver policy (relay mode, and direct mode since direct
    /// hosting).
    pub fn policy(&self) -> Result<Option<HostPolicy>> {
        read_json(&self.paths.policy())
    }

    fn manager(&self, runner: RunnerKind) -> Arc<dyn ServiceManager> {
        if runner == RunnerKind::AppHelper
            && let (Some(h), Some(_)) = (&self.helper, &self.helper_app)
        {
            return h.clone();
        }
        if let Some(m) = &self.manager {
            return m.clone();
        }
        Arc::from(service::manager_for(
            runner,
            &user_home(&self.home),
            &self.paths.dir,
        ))
    }

    fn manager_for_config(&self, config: &HostConfig) -> Arc<dyn ServiceManager> {
        self.manager(RunnerKind::parse(&config.runner).unwrap_or(RunnerKind::Auto))
    }

    /// The command line the service runs.
    pub fn service_spec(&self, config: &HostConfig) -> ServiceSpec {
        self.service_spec_for(config, CuaChannel::current())
    }

    /// [`Host::service_spec`] for a build of `channel`: the ports it
    /// serves on are the channel's, so Cua Spaces Local's host runs beside
    /// the Stable one.
    pub(crate) fn service_spec_for(&self, config: &HostConfig, channel: CuaChannel) -> ServiceSpec {
        let p = &self.paths;
        let s = |p: PathBuf| p.to_string_lossy().into_owned();
        let mut args: Vec<String> = match config.mode.as_str() {
            "relay" => vec![
                "join".into(),
                "--relay".into(),
                config.relay_url.clone().unwrap_or_default(),
                "--relay-token-file".into(),
                s(p.machine_token()),
                "--host-policy".into(),
                s(p.policy()),
                "--machine-id-file".into(),
                s(p.machine_id.clone()),
            ]
            .into_iter()
            // The relay keys returned at registration: the driver refuses a
            // relay that presents any other assertion key.
            .chain(
                p.relay_jwks()
                    .is_file()
                    .then(|| ["--relay-jwks".to_string(), s(p.relay_jwks())])
                    .into_iter()
                    .flatten(),
            )
            .collect(),
            _ if config.static_relay.is_some() => {
                // Direct and through a static relay at once: the driver
                // serves its direct listener and joins the relay, with the
                // policy that makes it a host on both.
                vec![
                    "join".into(),
                    "--relay".into(),
                    config.static_relay.clone().unwrap_or_default(),
                    "--relay-token-file".into(),
                    s(p.relay_token()),
                    "--host-policy".into(),
                    s(p.policy()),
                    "--machine-id-file".into(),
                    s(p.machine_id.clone()),
                    "--listen".into(),
                    config.listen.clone().unwrap_or_default(),
                ]
            }
            _ => {
                let mut args: Vec<String> = vec![
                    "serve".into(),
                    "--listen".into(),
                    config.listen.clone().unwrap_or_default(),
                ];
                // Hosting Spaces directly: the driver serves
                // HostSpacesService on its listener from the policy. Only
                // then, so a direct desktop setup keeps working with a
                // driver that predates the flag.
                if config.provide_spaces && p.policy().is_file() {
                    args.extend(["--direct-host-policy".to_string(), s(p.policy())]);
                }
                // Your tailnet vouches for who calls (Tailscale or
                // Headscale `whois`), per the policy's owner and shares.
                if config.tailnet && p.policy().is_file() {
                    args.extend(["--tailnet-identity-policy".to_string(), s(p.policy())]);
                }
                args
            }
        };
        args.extend([
            "--token-file".into(),
            s(p.env_token()),
            "--data-dir".into(),
            s(p.driver_data.clone()),
        ]);
        // A machine that does not share its desktop runs no desktop
        // services, no cua-driver and no /mcp at all: providing Spaces
        // never needs them (the policy refuses relayed calls to the rest).
        if !config.share_desktop {
            args.extend([
                "--no-desktop".into(),
                "--no-driver".into(),
                "--no-mcp".into(),
            ]);
        }
        let mut env = vec![("CUA_ENV_LOG".into(), "info".into())];
        // A Local build's host service is told its channel and ports, so it
        // runs beside a Stable one (3211 TCP, 3212 QUIC). Nothing is passed
        // for Stable: its service is the one it has always been, on
        // cua-spacesd's own defaults.
        //
        // The ports go twice. `--port` (the listener of a joined host;
        // `--listen` names a direct one) and `--quic-port` are on the
        // command line, where they show in `ps` and win; the environment
        // carries the same for everything the service starts, which reads
        // `CUA_ENV_PORT` and `CUA_CHANNEL`. The mesh media port is not
        // passed: it is 3212 for every channel, by design (a port of the
        // mesh's own network that guest Spaces and older cua-spacesd builds
        // fix, so two hosts on one Mac do not compete for it).
        let (ports, stable) = (channel.ports(), CuaChannel::Stable.ports());
        if ports.tcp != stable.tcp {
            args.extend(["--port".to_string(), ports.tcp.to_string()]);
            env.push((PORT_ENV.into(), ports.tcp.to_string()));
        }
        if ports.quic != stable.quic {
            args.extend(["--quic-port".to_string(), ports.quic.to_string()]);
            env.push((QUIC_PORT_ENV.into(), ports.quic.to_string()));
        }
        if channel != CuaChannel::Stable {
            env.push((CUA_CHANNEL_VAR.into(), channel.name().into()));
        }
        if let Some(mesh) = &config.mesh {
            env.push(("CUA_MESH".into(), mesh.clone()));
        }
        if let Some(usd) = &config.usd_per_hour {
            env.push((HOST_USD_PER_HOUR.into(), usd.clone()));
        }
        ServiceSpec {
            program: config.driver_bin.clone(),
            args,
            env,
            log_path: p.log(),
            working_dir: user_home(&self.home),
        }
    }

    /// Sets this machine up: installs the driver, registers with the relay
    /// (relay mode, as the account behind `tokens`) or generates an env
    /// token (direct mode), installs and starts the service. Re-running
    /// updates everything in place (and rotates the machine token).
    pub async fn setup(
        &self,
        opts: SetupOptions,
        tokens: &dyn AccountTokens,
    ) -> Result<HostStatus> {
        self.setup_staged(opts, tokens).await.map_err(|(_, e)| e)
    }

    /// [`Host::setup`], saying on failure which stage stopped it (usage
    /// data and the apps' messages tell "not signed in" from "the relay
    /// refused the account" from "the download failed").
    pub async fn setup_staged(
        &self,
        opts: SetupOptions,
        tokens: &dyn AccountTokens,
    ) -> std::result::Result<HostStatus, (SetupStage, Error)> {
        let mut stage = SetupStage::Preflight;
        self.setup_inner(opts, tokens, &mut stage)
            .await
            .map_err(|e| (stage, e))
    }

    async fn setup_inner(
        &self,
        opts: SetupOptions,
        tokens: &dyn AccountTokens,
        stage: &mut SetupStage,
    ) -> Result<HostStatus> {
        let runner = match (&self.manager, opts.runner) {
            (_, RunnerKind::Auto) if self.helper.is_some() => RunnerKind::AppHelper,
            (Some(m), _) => m.kind(),
            (None, r) => r.resolve(),
        };
        // The app's helper runs the cua-spacesd sealed in the app; its
        // launcher is the driver the config names (and the permission
        // checks ask).
        let helper_launcher = match runner {
            RunnerKind::AppHelper => Some(
                self.helper_app()
                    .map(|app| service::app_helper_launcher(&app))
                    .ok_or_else(|| {
                        Error::Service(format!(
                            "the host service runs inside the Cua Spaces app, which is not \
                             installed; install it, or run `{} host setup --runner launchd`",
                            crate::cli()
                        ))
                    })?,
            ),
            _ => None,
        };
        let name = opts
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(friendly_name);
        if !opts.share_desktop && !opts.provide_spaces {
            return Err(Error::InvalidArgument(
                "with neither the desktop nor Spaces there is nothing to share; turn on \
                 --desktop or --provide-spaces"
                    .into(),
            ));
        }
        // Before anything else: without a GUI (Aqua) session, bootstrapping
        // the LaunchAgent would fail later, often with no useful message
        // (set up over ssh with nobody logged in at the console). Checked
        // first so a doomed setup never registers with the relay,
        // downloads the driver, or leaves a config file behind.
        if runner == RunnerKind::Launchd || runner == RunnerKind::AppHelper {
            preflight::check(
                self.preflight_probe().as_ref(),
                helper_launcher
                    .as_deref()
                    .unwrap_or(&self.paths.driver_bin()),
            )?;
        }
        // Hosting Spaces on the plaintext direct listener: never on a
        // public address without --allow-any-address.
        if let HostMode::Direct { listen } = &opts.mode
            && opts.provide_spaces
            && let Some(warning) = crate::direct::check_listen(*listen, opts.allow_any_address)?
        {
            tracing::warn!("{warning}");
        }
        std::fs::create_dir_all(&self.paths.dir)?;
        // Every mode keeps a local env token (never sent to the relay).
        if read_secret(&self.paths.env_token()).map_or(true, |t| t.is_empty()) {
            write_secret(&self.paths.env_token(), &random_hex(24))?;
        }
        let mut config = HostConfig {
            name: name.clone(),
            runner: runner.as_str().into(),
            driver_bin: helper_launcher
                .clone()
                .unwrap_or_else(|| self.paths.driver_bin()),
            sharing: true,
            share_desktop: opts.share_desktop,
            provide_spaces: opts.provide_spaces,
            mesh: mesh_env(),
            usd_per_hour: host_usd_per_hour(),
            ..Default::default()
        };
        if opts.download_first && helper_launcher.is_none() {
            *stage = SetupStage::Download;
            install_driver(opts.driver_bin.as_deref(), &config.driver_bin).await?;
        }
        match &opts.mode {
            HostMode::Relay { url } => {
                let relay = RelayClient::new(url)?;
                *stage = SetupStage::Token;
                let token = tokens.access_token().await?;
                *stage = SetupStage::Register;
                let id = load_or_create_machine_id(&self.paths.machine_id)?;
                // Re-running setup proves it is this machine with its current
                // machine token; the account session is used only for this
                // call and never stored under the host directory.
                let current = read_secret(&self.paths.machine_token()).ok();
                let reg = relay
                    .register_as_machine(
                        &token,
                        current.as_deref(),
                        &RegisterRequest {
                            id: id.clone(),
                            name: name.clone(),
                            allow: opts.allow.clone(),
                            host: None,
                            meta: host_meta(opts.provide_spaces),
                        },
                    )
                    .await?;
                write_secret(&self.paths.machine_token(), &reg.machine_token)?;
                if reg.jwks.is_null() {
                    let _ = std::fs::remove_file(self.paths.relay_jwks());
                } else {
                    write_json(&self.paths.relay_jwks(), &reg.jwks)?;
                }
                write_json(
                    &self.paths.policy(),
                    &HostPolicy {
                        owner: reg.machine.owner.id.clone(),
                        owner_email: reg.machine.owner.email.clone(),
                        allow: opts.allow.clone(),
                        viewers: vec![],
                        trust_relay_allowlist: true,
                        sharing: true,
                        share_desktop: opts.share_desktop,
                        provide_spaces: opts.provide_spaces,
                        max_spaces: opts
                            .max_spaces
                            .unwrap_or(crate::provided::DEFAULT_MAX_SPACES),
                        max_macos_vms: crate::provided::MACOS_VM_LICENSE_LIMIT,
                        spaces_daemon: Some(self.spaces_daemon(opts.cua_bin.as_deref())),
                        direct: None,
                        tailnet_identity: false,
                        dedicated: opts.dedicated,
                        shares: true,
                        shares_gateway: Some(self.shares_gateway()),
                    },
                )?;
                config.mode = "relay".into();
                config.relay_url = Some(relay.base().to_string());
                config.machine_id = Some(reg.machine.id);
            }
            HostMode::Direct { listen } => {
                config.mode = "direct".into();
                config.listen = Some(listen.to_string());
                let _ = std::fs::remove_file(self.paths.machine_token());
                // The env token holder is the owner: the policy the driver
                // serves HostSpacesService from, with the same limits as a
                // relay host (four Spaces, two macOS VMs per Mac).
                let mut policy = self.direct_policy(&listen.to_string(), opts.cua_bin.as_deref());
                if let Some(d) = policy.direct.as_mut() {
                    d.allow_any_address = opts.allow_any_address;
                }
                policy.share_desktop = opts.share_desktop;
                policy.provide_spaces = opts.provide_spaces;
                policy.dedicated = opts.dedicated;
                if let Some(m) = opts.max_spaces {
                    policy.max_spaces = m;
                }
                if opts.tailnet {
                    // The tailnet's identity: this machine's tailnet user
                    // owns it, `--allow` names other tailnet users.
                    policy.tailnet_identity = true;
                    policy.owner_email = crate::direct::tailnet_login();
                    policy.allow = opts.allow.clone();
                    config.tailnet = true;
                }
                write_json(&self.paths.policy(), &policy)?;
                match &opts.static_relay {
                    Some(r) => {
                        write_secret(&self.paths.relay_token(), &r.token)?;
                        config.static_relay = Some(r.url.trim_end_matches('/').to_string());
                        config.machine_id =
                            Some(load_or_create_machine_id(&self.paths.machine_id)?);
                    }
                    None => {
                        let _ = std::fs::remove_file(self.paths.relay_token());
                    }
                }
            }
        }
        if !opts.download_first && helper_launcher.is_none() {
            *stage = SetupStage::Download;
            install_driver(opts.driver_bin.as_deref(), &config.driver_bin).await?;
        }
        // Before a service is installed to restart it forever: a binary
        // whose shared libraries are missing never starts.
        let missing = missing_libraries(&config.driver_bin);
        if !missing.is_empty() {
            return Err(Error::Service(missing_libraries_message(&missing)));
        }
        *stage = SetupStage::Service;
        write_json(&self.paths.config(), &config)?;
        if runner == RunnerKind::AppHelper {
            // Never two services: the old LaunchAgent (and its binary)
            // goes before the helper starts.
            self.retire_legacy()?;
        }
        let manager = self.manager(runner);
        manager.install(&self.service_spec(&config))?;
        manager.start()?;
        if let Err(e) = confirm_running(manager.as_ref(), &self.paths.log()).await {
            let _ = manager.stop();
            return Err(e);
        }
        let _ = crate::provided::audit(
            &self.paths.dir,
            "config",
            "local",
            &config.name,
            &if opts.dedicated {
                format!(
                    "{} dedicated=on",
                    settings_detail(config.share_desktop, config.provide_spaces)
                )
            } else {
                settings_detail(config.share_desktop, config.provide_spaces)
            },
        );
        self.status().await
    }

    /// The policy of a host in direct mode listening on `listen`: the env
    /// token holder (`local`) owns it; Spaces off until set.
    fn direct_policy(&self, listen: &str, cua_bin: Option<&Path>) -> HostPolicy {
        HostPolicy {
            owner: "local".into(),
            owner_email: None,
            allow: vec![],
            viewers: vec![],
            // No relay vouches for anyone in direct mode.
            trust_relay_allowlist: false,
            sharing: true,
            share_desktop: true,
            provide_spaces: false,
            max_spaces: crate::provided::DEFAULT_MAX_SPACES,
            max_macos_vms: crate::provided::MACOS_VM_LICENSE_LIMIT,
            spaces_daemon: Some(self.spaces_daemon(cua_bin)),
            direct: Some(DirectHosting {
                listen: listen.to_string(),
                allow_any_address: false,
            }),
            tailnet_identity: false,
            dedicated: false,
            shares: false,
            shares_gateway: None,
        }
    }

    /// The command to run on a laptop to add this machine (direct mode
    /// only): `cua spaces add <address> --host --name <name> --token
    /// <token>` (`--host` when it provides Spaces).
    /// The address is the listen address, or for an every-interface bind
    /// this machine's Tailscale address, else its LAN address. Holds the
    /// env token: show it only to the owner.
    pub fn pairing_command(&self) -> Result<Option<String>> {
        let Some(config) = self.config()? else {
            return Ok(None);
        };
        if config.mode != "direct" {
            return Ok(None);
        }
        let Some(listen) = config
            .listen
            .as_deref()
            .and_then(|l| l.parse::<SocketAddr>().ok())
        else {
            return Ok(None);
        };
        let ip = if listen.ip().is_unspecified() {
            crate::direct::tailscale_ip()
                .or_else(lan_ip)
                .unwrap_or(listen.ip())
        } else {
            listen.ip()
        };
        if config.tailnet {
            // Tailnet identity: whoever the policy names adds it without a
            // token (their tailnet login is the credential).
            return Ok(Some(crate::direct::tailnet_pairing_command(
                &crate::direct::authority(ip, listen.port()),
                &config.name,
                config.provide_spaces,
            )));
        }
        let token = read_secret(&self.paths.env_token())?;
        Ok(Some(crate::direct::pairing_command(
            &crate::direct::authority(ip, listen.port()),
            &config.name,
            &token,
            config.provide_spaces,
        )))
    }

    /// How this machine's driver reaches its cua daemon (the one that
    /// creates provided Spaces): the daemon socket in this cua home, and
    /// the `cua` CLI that starts it.
    fn spaces_daemon(&self, cua_bin: Option<&Path>) -> SpacesDaemon {
        SpacesDaemon {
            socket: self.home.join("cua.sock").to_string_lossy().into_owned(),
            cua_home: self.home.to_string_lossy().into_owned(),
            cua_bin: find_cua_bin(cua_bin).map(|p| p.to_string_lossy().into_owned()),
        }
    }

    /// The socket of the cua daemon's share gateway in this cua home.
    pub fn shares_gateway(&self) -> String {
        self.home
            .join("share")
            .join("gateway.sock")
            .to_string_lossy()
            .into_owned()
    }

    /// Turns on forwarding of `/share/...` to the daemon's share gateway
    /// (`cua share`). The driver reads the policy on every request, so no
    /// restart is needed. Only a machine on the relay can share links: a
    /// machine that is not set up (or is in direct mode without a relay)
    /// gets an error that says how to join one. Returns whether the policy
    /// changed.
    pub fn enable_shares(&self) -> Result<bool> {
        let config = self.config()?.ok_or_else(|| {
            Error::NotFound(format!(
                "this machine is not on your cua.ai relay; set it up with `{} host setup`",
                crate::cli()
            ))
        })?;
        if config.mode != "relay" {
            return Err(Error::InvalidArgument(format!(
                "this machine is not on your cua.ai relay (direct mode); links need the relay: `{} host setup`",
                crate::cli()
            )));
        }
        let mut policy = self.policy()?.ok_or_else(|| {
            Error::NotFound(format!(
                "this machine has no host policy; run `{} host setup`",
                crate::cli()
            ))
        })?;
        let gateway = self.shares_gateway();
        if policy.shares && policy.shares_gateway.as_deref() == Some(gateway.as_str()) {
            return Ok(false);
        }
        policy.shares = true;
        policy.shares_gateway = Some(gateway);
        write_json(&self.paths.policy(), &policy)?;
        Ok(true)
    }

    /// Changes this machine's Spaces settings (`cua host config`, the app's
    /// This machine settings). The driver's policy takes effect on the next
    /// relayed call; turning the desktop on or off also restarts the
    /// service, so a machine that does not share its desktop never runs its
    /// desktop services. Every change is a line in the Spaces audit.
    pub async fn configure(&self, change: HostSettingsChange) -> Result<HostStatus> {
        let mut config = self.require_config()?;
        let desktop = change.share_desktop.unwrap_or(config.share_desktop);
        let provide = change.provide_spaces.unwrap_or(config.provide_spaces);
        let relay = config.mode == "relay";
        // Both off is a valid choice: there is nothing to share, so sharing
        // stops (below) and the setup stays; turning one on again and
        // Resume sharing (`cua host start`) share again.
        let nothing = !desktop && !provide;
        if let Some(m) = change.max_macos_vms
            && m > crate::provided::MACOS_VM_LICENSE_LIMIT
        {
            return Err(Error::InvalidArgument(format!(
                "at most {} macOS VMs: {}",
                crate::provided::MACOS_VM_LICENSE_LIMIT,
                crate::provided::MACOS_LIMIT_REASON
            )));
        }
        // A direct host that starts providing Spaces needs a policy (one
        // set up before direct hosting existed has none).
        let policy = match self.policy()? {
            Some(p) => Some(p),
            None if !relay => config
                .listen
                .as_deref()
                .map(|l| self.direct_policy(l, change.cua_bin.as_deref())),
            None => None,
        };
        if !relay
            && provide
            && let Some(listen) = config
                .listen
                .as_deref()
                .and_then(|l| l.parse::<SocketAddr>().ok())
        {
            let any = policy
                .as_ref()
                .and_then(|p| p.direct.as_ref())
                .is_some_and(|d| d.allow_any_address);
            crate::direct::check_listen(listen, any)?;
        }
        // The driver's flags change with the desktop, and in direct mode
        // with Spaces (it serves HostSpacesService only then).
        // A service that should run but is down (a start that failed
        // before) starts again too, so trying the change again repairs it.
        let down = config.sharing && !self.manager_for_config(&config).state().running;
        // Paused while signed out: the service stays uninstalled (a new
        // plist would start it again at login); resuming installs it with
        // these settings.
        let restart = !config.paused_signed_out
            && !nothing
            && (down
                || desktop != config.share_desktop
                || (!relay && provide != config.provide_spaces));
        config.share_desktop = desktop;
        config.provide_spaces = provide;
        if let Some(mut p) = policy {
            p.share_desktop = desktop;
            p.provide_spaces = provide;
            if let Some(m) = change.max_spaces {
                p.max_spaces = m;
            }
            if let Some(m) = change.max_macos_vms {
                p.max_macos_vms = m;
            }
            if p.spaces_daemon.is_none() || change.cua_bin.is_some() {
                p.spaces_daemon = Some(self.spaces_daemon(change.cua_bin.as_deref()));
            }
            write_json(&self.paths.policy(), &p)?;
        }
        write_json(&self.paths.config(), &config)?;
        crate::provided::audit(
            &self.paths.dir,
            "config",
            "local",
            &config.name,
            &settings_detail(desktop, provide),
        )?;
        if restart {
            let manager = self.manager_for_config(&config);
            manager.install(&self.service_spec(&config))?;
            if config.sharing {
                manager.start()?;
            }
        }
        if nothing && config.sharing {
            // Stop advertising: the relay lists it as not sharing (other
            // devices say it shares neither its desktop nor Spaces).
            config.stopped_for_nothing = true;
            write_json(&self.paths.config(), &config)?;
            return self.stop_sharing().await;
        }
        if !nothing && !config.sharing && config.stopped_for_nothing {
            // It stopped only because there was nothing to share: there is
            // again, so it shares again (a stop the owner asked for stays).
            config.stopped_for_nothing = false;
            write_json(&self.paths.config(), &config)?;
            return self.start_sharing().await;
        }
        self.status().await
    }

    /// Current state: config, service, and (relay mode) the relay's view
    /// with the connected clients.
    pub async fn status(&self) -> Result<HostStatus> {
        let Some(config) = self.config()? else {
            return Ok(HostStatus::default());
        };
        let policy = self.policy()?;
        let manager = self.manager_for_config(&config);
        let mut status = HostStatus {
            configured: true,
            mode: Some(config.mode.clone()),
            relay_url: config.relay_url.clone(),
            direct_url: config.listen.as_deref().and_then(direct_url),
            env_token_path: Some(self.paths.env_token().to_string_lossy().into_owned()),
            machine_id: config.machine_id.clone(),
            name: Some(config.name.clone()),
            sharing: policy.as_ref().map_or(config.sharing, |p| p.sharing),
            share_desktop: config.share_desktop,
            provide_spaces: config.provide_spaces,
            max_spaces: policy.as_ref().map_or(0, |p| p.max_spaces),
            max_macos_vms: policy
                .as_ref()
                .map_or(0, |p| p.spaces_settings().max_macos_vms),
            provided_spaces: crate::provided::load_provided(&self.paths.dir)
                .unwrap_or_default()
                .iter()
                .map(crate::provided::ProvidedSpace::public)
                .collect(),
            service: manager.state(),
            allow: policy.as_ref().map(|p| p.allow.clone()).unwrap_or_default(),
            permissions: self.missing_permissions(&config),
            ..Default::default()
        };
        if config.mode == "direct" {
            status.sharing = config.sharing && status.service.running;
        }
        if config.mode == "relay"
            && let Some(p) = policy.as_ref()
        {
            status.owner = Some(p.owner.clone()).filter(|o| !o.is_empty());
            status.owner_email = p.owner_email.clone();
        }
        status.paused_signed_out = config.paused_signed_out;
        if cfg!(target_os = "macos") || self.helper.is_some() {
            status.helper_available = self.helper_app().is_some();
            status.legacy_service = self.legacy().found().present();
        }
        if let (Some(url), Some(id)) = (&config.relay_url, &config.machine_id) {
            match recall_relay_view(url, id, self.relay_machine(url, id).await) {
                Ok(m) => {
                    status.online = Some(m.online);
                    // Paused: nobody can reach it, whatever the relay's
                    // sharing switch says.
                    status.sharing = m.sharing && !config.paused_signed_out;
                    status.clients = m.clients;
                    if !m.allow.is_empty() {
                        status.allow = m.allow;
                    }
                }
                Err(e) => status.error = Some(e.to_string()),
            }
        }
        self.add_access(&mut status, now_ms());
        let audit = crate::provided::read_audit(&self.paths.dir, crate::access::RECENT_LIMIT);
        status.spaces_audit = audit.recent;
        status.spaces_audit_error = audit.error;
        Ok(status)
    }

    /// The permissions left to grant: every one macOS needs, less the ones
    /// the driver reports granted (asked afresh on each status, so a grant
    /// shows without restarting anything). All of them when it cannot say.
    fn missing_permissions(&self, config: &HostConfig) -> Vec<PermissionHint> {
        // Screen Recording and Accessibility let people you share with see
        // and control this desktop. A machine that only provides Spaces
        // (a spare Mac) keeps its desktop private and runs Spaces in VMs,
        // so it needs neither.
        if !config.share_desktop {
            return vec![];
        }
        let hints = permission_hints(std::env::consts::OS, &config.driver_bin);
        if hints.is_empty() {
            return hints;
        }
        match self.preflight_probe().permission_status(&config.driver_bin) {
            Some((screen, ax)) => hints
                .into_iter()
                .filter(|h| match h.id.as_str() {
                    "screen-recording" => !screen,
                    "accessibility" => !ax,
                    _ => true,
                })
                .collect(),
            None => hints,
        }
    }

    /// Fills the recent accesses from the driver's log and adds the callers
    /// active right now to `clients`: every one in direct mode (there is no
    /// relay presence), and in relay mode the ones the relay cannot see
    /// (token and viewer callers) or all of them when the relay did not
    /// answer. Only connections count ([`crate::access::AccessKind`]): a
    /// relay client whose calls this machine logged as background probes,
    /// thumbnails or refusals only is not "connected now".
    fn add_access(&self, status: &mut HostStatus, now_ms: u64) {
        let report = crate::access::read(&self.paths.access_log(), crate::access::STATUS_LIMIT);
        status.clients.retain(|c| {
            crate::access::relay_client_connected(
                &report.recent,
                &c.id,
                c.since.saturating_mul(1000),
                now_ms,
            )
            .unwrap_or(true)
        });
        let relay_answered = status.online.is_some();
        let active: Vec<ConnectedClient> = crate::access::active(&report.recent, now_ms)
            .into_iter()
            .filter(|r| !(relay_answered && r.via == "relay"))
            .map(|r| ConnectedClient {
                id: format!("{}:{}", r.via, r.who),
                email: None,
                name: Some(r.who.clone()),
                streams: 0,
                since: r.at_ms / 1000,
            })
            .collect();
        status.clients.extend(active);
        status.recent_access = report.recent;
        status.access_log_error = report.error;
    }

    async fn relay_machine(&self, url: &str, id: &str) -> Result<crate::Machine> {
        let token = read_secret(&self.paths.machine_token())?;
        RelayClient::new(url)?.machine(&token, id).await
    }

    /// Takes `name` as this machine's display name when `machine_id` is
    /// this machine's relay registration (after the owner renamed it on
    /// the relay from an enrolled device). Returns whether it did: another
    /// machine, or a machine not set up, is left alone.
    pub fn adopt_name(&self, machine_id: &str, name: &str) -> Result<bool> {
        let Some(mut config) = self.config()? else {
            return Ok(false);
        };
        if config.machine_id.as_deref() != Some(machine_id) || name.trim().is_empty() {
            return Ok(false);
        }
        config.name = name.trim().to_string();
        write_json(&self.paths.config(), &config)?;
        Ok(true)
    }

    fn require_config(&self) -> Result<HostConfig> {
        self.config()?.ok_or_else(|| {
            Error::NotFound(format!(
                "this computer is not set up for access (run `{} host setup`)",
                crate::cli()
            ))
        })
    }

    fn set_policy_sharing(&self, sharing: bool) -> Result<()> {
        if let Some(mut p) = self.policy()? {
            p.sharing = sharing;
            write_json(&self.paths.policy(), &p)?;
        }
        Ok(())
    }

    /// "Stop sharing": relay mode cuts every client at the relay and refuses
    /// new ones (and the driver's policy refuses them too); direct mode
    /// stops the service.
    pub async fn stop_sharing(&self) -> Result<HostStatus> {
        let mut config = self.require_config()?;
        match (&config.relay_url, &config.machine_id) {
            (Some(url), Some(id)) => {
                // The local policy first: even if the relay is unreachable the
                // driver refuses relayed clients from now on.
                self.set_policy_sharing(false)?;
                let token = read_secret(&self.paths.machine_token())?;
                RelayClient::new(url)?.stop_sharing(&token, id).await?;
            }
            _ => {
                self.manager_for_config(&config).stop()?;
            }
        }
        config.sharing = false;
        write_json(&self.paths.config(), &config)?;
        self.status().await
    }

    /// Undoes [`Host::stop_sharing`]. Refused while neither the desktop
    /// nor Spaces is on: there is nothing to share.
    pub async fn start_sharing(&self) -> Result<HostStatus> {
        let mut config = self.require_config()?;
        if !config.share_desktop && !config.provide_spaces {
            return Err(Error::InvalidArgument(format!(
                "there is nothing to share: turn on the desktop or Spaces first \
                 (`{} host config --desktop on` or `--provide-spaces on`)",
                crate::cli()
            )));
        }
        match (&config.relay_url, &config.machine_id) {
            (Some(url), Some(id)) => {
                let token = read_secret(&self.paths.machine_token())?;
                RelayClient::new(url)?.start_sharing(&token, id).await?;
                self.set_policy_sharing(true)?;
                // An explicit start (`cua host start`) ends a pause too.
                config.paused_signed_out = false;
                config.stopped_for_nothing = false;
                // The service stays up while sharing is stopped; one that
                // went down (a failed start) is started again here.
                let manager = self.manager_for_config(&config);
                if !manager.state().running {
                    manager.install(&self.service_spec(&config))?;
                    manager.start()?;
                }
            }
            _ => {
                self.manager_for_config(&config).start()?;
            }
        }
        config.sharing = true;
        write_json(&self.paths.config(), &config)?;
        self.status().await
    }

    /// Pauses relay sharing because nobody is signed in to the owner's
    /// account here (signed out, or a session that can no longer refresh):
    /// the driver's policy refuses relayed clients, and the service is
    /// stopped and uninstalled, so this machine leaves the relay (offline
    /// for the account) and does not join it again at login. The setup
    /// stays (machine token, policy, config) for
    /// [`Host::resume_signed_in`]. Direct mode, and a machine that is not
    /// set up, are left alone: they do not use the account.
    pub async fn pause_signed_out(&self) -> Result<HostStatus> {
        let Some(mut config) = self.config()? else {
            return self.status().await;
        };
        if config.mode != "relay" {
            return self.status().await;
        }
        // The flag first: a settings change racing this one never installs
        // the service again.
        config.paused_signed_out = true;
        write_json(&self.paths.config(), &config)?;
        // Even if something starts the driver anyway, it refuses relayed
        // clients. `config.sharing` keeps the owner's own choice.
        self.set_policy_sharing(false)?;
        self.manager_for_config(&config).uninstall()?;
        let _ = crate::provided::audit(
            &self.paths.dir,
            "config",
            "local",
            &config.name,
            "relay sharing paused: signed out",
        );
        self.status().await
    }

    /// Undoes [`Host::pause_signed_out`] once the owner is signed in again:
    /// `account` is the signed-in account's id (or email). Another account
    /// is refused: it cannot use this registration, and sharing the old
    /// account's machine under it would be wrong
    /// ([`Host::resume_or_switch_account`] registers it again for that
    /// account instead). Sharing comes back as the owner
    /// left it (stopped stays stopped, but online).
    pub async fn resume_signed_in(&self, account: &str) -> Result<HostStatus> {
        let mut config = self.require_config()?;
        if !config.paused_signed_out {
            return self.status().await;
        }
        let policy = self.policy()?;
        if let Some(p) = &policy
            && !owned_by(p, account)
        {
            return Err(Error::InvalidArgument(
                "this computer was set up for another Cua account; sign in to that \
                 account, or register this computer again for this one"
                    .into(),
            ));
        }
        if let Some(mut p) = policy {
            p.sharing = config.sharing;
            write_json(&self.paths.policy(), &p)?;
        }
        // Stopped stays stopped, but online: a registration made while the
        // owner had stopped sharing starts as sharing on the relay, so the
        // stop is sent again (it changes nothing when the relay has it).
        if !config.sharing
            && let (Some(url), Some(id)) = (&config.relay_url, &config.machine_id)
            && let Ok(token) = read_secret(&self.paths.machine_token())
        {
            RelayClient::new(url)?.stop_sharing(&token, id).await?;
        }
        let manager = self.manager_for_config(&config);
        if let Err(e) = manager
            .install(&self.service_spec(&config))
            .and_then(|()| manager.start())
        {
            // Still paused (and refusing clients), so the next sign-in
            // check tries again instead of finding a sharing host that is
            // not running.
            self.set_policy_sharing(false)?;
            return Err(e);
        }
        config.paused_signed_out = false;
        write_json(&self.paths.config(), &config)?;
        let _ = crate::provided::audit(
            &self.paths.dir,
            "config",
            "local",
            &config.name,
            "relay sharing resumed: signed in",
        );
        self.status().await
    }

    /// [`Host::resume_signed_in`] that also follows a switch of account:
    /// when `account` is not the machine's owner, this computer is
    /// registered again for the signed-in account (behind `tokens`), so
    /// signing in as someone else needs no manual remove and set up. The
    /// relay still decides: it only accepts the registration of the account
    /// that signed in. Direct mode is left alone.
    pub async fn resume_or_switch_account(
        &self,
        account: &str,
        tokens: &dyn AccountTokens,
    ) -> Result<HostStatus> {
        let config = self.require_config()?;
        if config.mode == "relay"
            && let Some(p) = self.policy()?
            && !owned_by(&p, account)
        {
            return self.switch_account(tokens).await;
        }
        self.resume_signed_in(account).await
    }

    /// Registers this computer again for the account behind `tokens`, which
    /// is not the one it was set up for. The old registration belongs to
    /// the previous account (the relay refuses its id, and the machine's
    /// mesh key was bound to it), so this computer registers under a machine
    /// id of the new account and takes a new mesh key; its name, what it
    /// shares and the Provide Spaces setting stay. The previous account's
    /// allowlist is not carried over. While it still runs Spaces for the
    /// previous account it stays paused instead (they are not handed to
    /// another account). The registration it leaves is kept
    /// (see [`Registrations`]) for when that account signs in again, so
    /// switching back reuses it: no machines pile up on the relay, and what
    /// is tied to the machine (its provided Spaces) stays with it.
    async fn switch_account(&self, tokens: &dyn AccountTokens) -> Result<HostStatus> {
        let mut config = self.require_config()?;
        if config.relay_url.is_none() || self.policy()?.is_none() {
            return Err(Error::InvalidArgument(
                "this computer is not set up on a relay".into(),
            ));
        }
        // Off the relay first, as when signing out: if the new registration
        // fails, the previous account's access stays closed and the next
        // reconcile tries again (paused, with another account signed in).
        if !config.paused_signed_out {
            self.pause_signed_out().await?;
            config = self.require_config()?;
        }
        let (Some(url), Some(mut policy)) = (config.relay_url.clone(), self.policy()?) else {
            return Err(Error::InvalidArgument(
                "this computer is not set up on a relay".into(),
            ));
        };
        // The Spaces it provides were made for the previous account's
        // devices and are tied to its registration: they are not handed to
        // another account. It stays paused until they are deleted (or that
        // account signs in again).
        let provided = crate::provided::load_provided(&self.paths.dir)?;
        if !provided.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "this computer still runs {} Space{} for the account that was signed in \
                 before; delete {} (or sign in to that account again) to share this \
                 computer with the account you are signed in to now",
                provided.len(),
                if provided.len() == 1 { "" } else { "s" },
                if provided.len() == 1 { "it" } else { "them" },
            )));
        }
        let relay = RelayClient::new(&url)?;
        let token = tokens.access_token().await?;
        let mut kept = self.registrations();
        // The registration being left, kept for its account.
        if !policy.owner.is_empty()
            && let (Some(id), Ok(machine_token)) = (
                config.machine_id.clone(),
                read_secret(&self.paths.machine_token()),
            )
        {
            kept.insert(
                policy.owner.clone(),
                StoredRegistration {
                    machine_id: id,
                    machine_token,
                },
            );
        }
        let request = |id: String| RegisterRequest {
            id,
            name: config.name.clone(),
            allow: vec![],
            host: None,
            meta: host_meta(config.provide_spaces),
        };
        // This account's earlier registration of this computer, if any (the
        // token says whose it is when it can; the relay refuses the
        // others): else, or when the relay no longer takes it, a new
        // machine id.
        let sub = crate::device::token_subject(&token);
        let earlier: Vec<StoredRegistration> = match sub.as_ref().and_then(|s| kept.get(s)) {
            Some(r) => vec![r.clone()],
            None => kept.values().cloned().collect(),
        };
        let mut reg = None;
        for r in earlier {
            match relay
                .register_as_machine(&token, Some(&r.machine_token), &request(r.machine_id))
                .await
            {
                Ok(r) => {
                    reg = Some(r);
                    break;
                }
                Err(
                    Error::Unauthenticated(_)
                    | Error::PermissionDenied(_)
                    | Error::NotFound(_)
                    | Error::Conflict(_),
                ) => {}
                Err(e) => return Err(e),
            }
        }
        let reg = match reg {
            Some(r) => r,
            None => {
                relay
                    .register_as_machine(&token, None, &request(random_hex(16)))
                    .await?
            }
        };
        // The new registration is kept before anything else can fail, so a
        // retry reuses it instead of making another machine.
        kept.insert(
            reg.machine.owner.id.clone(),
            StoredRegistration {
                machine_id: reg.machine.id.clone(),
                machine_token: reg.machine_token.clone(),
            },
        );
        self.save_registrations(&kept)?;
        // The relay's sharing state follows this computer's own choice
        // (a reused registration keeps whatever it had under its account;
        // a new one starts as sharing). Before anything local changes, so a
        // failure leaves this paused with the old owner and the next check
        // tries it all again.
        if config.sharing {
            relay
                .start_sharing(&reg.machine_token, &reg.machine.id)
                .await?;
        } else {
            relay
                .stop_sharing(&reg.machine_token, &reg.machine.id)
                .await?;
        }
        // The active registration is not kept; the others are.
        kept.remove(&reg.machine.owner.id);
        self.save_registrations(&kept)?;

        write_secret(&self.paths.machine_id, &format!("{}\n", reg.machine.id))?;
        write_secret(&self.paths.machine_token(), &reg.machine_token)?;
        if reg.jwks.is_null() {
            let _ = std::fs::remove_file(self.paths.relay_jwks());
        } else {
            write_json(&self.paths.relay_jwks(), &reg.jwks)?;
        }
        // The old machine's mesh key is bound to its machine id: a new
        // machine takes a new one, which the service makes on its start.
        let _ = std::fs::remove_file(self.paths.driver_data.join("mesh").join("identity.json"));
        policy.owner = reg.machine.owner.id.clone();
        policy.owner_email = reg.machine.owner.email.clone();
        policy.allow = vec![];
        policy.viewers = vec![];
        // Still paused (not sharing) until the service is up.
        policy.sharing = false;
        write_json(&self.paths.policy(), &policy)?;
        config.machine_id = Some(reg.machine.id.clone());
        write_json(&self.paths.config(), &config)?;
        // Registered for the new account and paused: resuming installs and
        // starts the service with the new registration (and keeps a stopped
        // host stopped on the relay), and is what a retry does if that fails.
        self.resume_signed_in(&reg.machine.owner.id).await
    }

    /// The registrations of this computer under accounts that are not the
    /// signed-in one (`<home>/host/registrations.json`, 0600).
    fn registrations(&self) -> Registrations {
        read_secret(&self.paths.dir.join(REGISTRATIONS_FILE))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save_registrations(&self, kept: &Registrations) -> Result<()> {
        let text = serde_json::to_string(kept).map_err(|e| Error::Internal(e.to_string()))?;
        write_secret(&self.paths.dir.join(REGISTRATIONS_FILE), &text)
    }

    /// Renames the machine and/or replaces its allowlist (relay mode; the
    /// account must own it).
    pub async fn update(
        &self,
        name: Option<String>,
        allow: Option<Vec<String>>,
        tokens: &dyn AccountTokens,
    ) -> Result<HostStatus> {
        let mut config = self.require_config()?;
        let (Some(url), Some(id)) = (config.relay_url.clone(), config.machine_id.clone()) else {
            return Err(Error::InvalidArgument(
                "name and allowlist live on the relay; this computer is in direct mode".into(),
            ));
        };
        let token = tokens.access_token().await?;
        RelayClient::new(&url)?
            .patch(
                &token,
                &id,
                &MachinePatch {
                    name: name.clone(),
                    allow: allow.clone(),
                    viewers: None,
                    sharing: None,
                    meta: None,
                },
            )
            .await?;
        if let Some(n) = name {
            config.name = n;
            write_json(&self.paths.config(), &config)?;
        }
        if let (Some(a), Some(mut p)) = (allow, self.policy()?) {
            p.allow = a;
            write_json(&self.paths.policy(), &p)?;
        }
        self.status().await
    }

    /// Mirrors an allowlist changed on the relay into the driver policy
    /// (when this machine is set up in relay mode).
    pub fn sync_policy_allow(&self, allow: &[String]) -> Result<()> {
        if let Some(mut p) = self.policy()? {
            p.allow = allow.to_vec();
            write_json(&self.paths.policy(), &p)?;
        }
        Ok(())
    }

    /// Mirrors both share lists changed on the relay into the driver
    /// policy, so the driver enforces a view-only share even before the
    /// relay's next assertion (when this machine is set up in relay mode).
    pub fn sync_policy_shares(&self, allow: &[String], viewers: &[String]) -> Result<()> {
        if let Some(mut p) = self.policy()? {
            p.allow = allow.to_vec();
            p.viewers = viewers.to_vec();
            write_json(&self.paths.policy(), &p)?;
        }
        Ok(())
    }

    /// Unregisters from the relay (best effort: a relay that cannot be
    /// reached is reported after the local cleanup), uninstalls the service
    /// and deletes `<home>/host`. The machine id is kept so a later setup
    /// reuses it.
    pub async fn remove(&self) -> Result<()> {
        self.remove_inner(false).await
    }

    /// [`Self::remove`] for an uninstall of the app: the Spaces this machine
    /// provides do not stop it. Their relay machines (each Space joined the
    /// relay as its own) are unregistered too, so nothing stays reachable
    /// through the relay; the Spaces themselves are left as they are for
    /// the caller to turn off ([`Self::provided_spaces`]).
    pub async fn remove_for_uninstall(&self) -> Result<()> {
        self.remove_inner(true).await
    }

    /// The Spaces this host provides to the owner's other devices.
    pub fn provided_spaces(&self) -> Result<Vec<crate::provided::ProvidedSpace>> {
        crate::provided::load_provided(&self.paths.dir)
    }

    // ------------------------------------------------- the app's helper

    /// Moves the host service from the old hand-written LaunchAgent to the
    /// Cua Spaces app (its login item, under the app's own name), keeping the machine
    /// (config, machine id and token, policy, provided Spaces); see
    /// [`helper_migration`]. `app_version` is the app's (a failed move is
    /// not tried again by itself with the same version); `force` tries
    /// again anyway. Never fails for a move that did not work: the old
    /// agent is put back and the outcome says so (`rolled-back`).
    pub async fn migrate_service(
        &self,
        force: bool,
        app_version: &str,
    ) -> Result<ServiceMigration> {
        let config = self.config()?;
        let app = self.helper_app();
        let legacy = self.legacy();
        let facts = MigrationFacts {
            helper_available: app.is_some(),
            configured: config.is_some(),
            runner: config
                .as_ref()
                .and_then(|c| RunnerKind::parse(&c.runner).ok()),
            paused: config.as_ref().is_some_and(|c| c.paused_signed_out),
            legacy: legacy.found(),
            legacy_binary: self.paths.driver_bin().is_file(),
            last: read_json(&self.paths.dir.join(helper_migration::RECORD_FILE))
                .ok()
                .flatten(),
            app_version: app_version.to_string(),
            force,
        };
        let plan = helper_migration::plan(&facts);
        let (Some(app), Some(mut config)) = (app, config) else {
            if let MigrationPlan::Nothing(why) = plan {
                return Ok(ServiceMigration::new("not-needed", why, false));
            }
            return Ok(ServiceMigration::new(
                "not-needed",
                "nothing to move",
                false,
            ));
        };
        let launcher = service::app_helper_launcher(&app);
        match plan {
            MigrationPlan::Nothing(why) => Ok(ServiceMigration::new("not-needed", why, false)),
            MigrationPlan::KeepAfterFailure => Ok(ServiceMigration::new(
                "kept",
                format!(
                    "moving into the Cua Spaces app failed before with this version; the old host \
                     service keeps running (`{} host migrate --force` tries again)",
                    crate::cli()
                ),
                false,
            )),
            MigrationPlan::RemoveLeftovers => {
                self.retire_legacy()?;
                Ok(ServiceMigration::new(
                    "cleaned-up",
                    "removed what was left of the old host service",
                    false,
                ))
            }
            MigrationPlan::SwitchPaused => {
                config.runner = RunnerKind::AppHelper.as_str().into();
                config.driver_bin = launcher;
                write_json(&self.paths.config(), &config)?;
                self.retire_legacy()?;
                self.record_migration("done", app_version, None);
                Ok(ServiceMigration::new(
                    "switched",
                    "the host service will run inside the Cua Spaces app when sharing resumes",
                    config.share_desktop,
                ))
            }
            MigrationPlan::Move => self.move_to_helper(config, &app, legacy, app_version).await,
        }
    }

    async fn move_to_helper(
        &self,
        config: HostConfig,
        app: &Path,
        legacy: Arc<dyn LegacyService>,
        app_version: &str,
    ) -> Result<ServiceMigration> {
        let helper = self.helper_manager(app);
        let mut next = config.clone();
        next.runner = RunnerKind::AppHelper.as_str().into();
        next.driver_bin = service::app_helper_launcher(app);
        // Direct mode with sharing stopped keeps the service stopped.
        let run = config.mode == "relay" || config.sharing;
        let before = self.relay_session(&config).await;
        let found = legacy.found();
        self.record_migration("in-progress", app_version, None);

        // 1. The old agent out (its plist kept aside, so a restart in the
        //    middle never runs two services).
        if let Err(e) = legacy.bootout() {
            let why = format!("the old host service did not stop: {e}");
            self.record_migration("rolled-back", app_version, Some(&why));
            return Ok(ServiceMigration::new("rolled-back", why, false));
        }
        let plist = legacy.plist();
        let backup = self.paths.dir.join(helper_migration::LEGACY_PLIST_BACKUP);
        let moved = plist.is_file() && move_file(&plist, &backup).is_ok();

        // 2. The helper in, and proved.
        let mut result = helper.install(&self.service_spec(&next));
        if result.is_ok() {
            result = if run { helper.start() } else { helper.stop() };
        }
        if result.is_ok() && run {
            result = self.verify_helper(helper.as_ref(), &next, before).await;
        }

        match result {
            Ok(()) => {
                write_json(&self.paths.config(), &next)?;
                let _ = std::fs::remove_file(&backup);
                self.remove_legacy_binary();
                self.record_migration("done", app_version, None);
                let _ = crate::provided::audit(
                    &self.paths.dir,
                    "config",
                    "local",
                    &next.name,
                    "host service moved into the Cua Spaces app",
                );
                let needs = next.share_desktop && !self.missing_permissions(&next).is_empty();
                Ok(ServiceMigration::new(
                    "migrated",
                    "the host service now runs inside the Cua Spaces app; this machine kept its id, \
                     its registration and its Spaces",
                    needs,
                ))
            }
            Err(e) => {
                let _ = helper.uninstall();
                if moved {
                    let _ = move_file(&backup, &plist);
                }
                let mut why = format!("the Cua Spaces app's host service did not take over: {e}");
                if found.loaded
                    && let Err(e) = legacy.bootstrap()
                {
                    why.push_str(&format!(
                        "; restarting the old host service failed too: {e}"
                    ));
                }
                self.record_migration("rolled-back", app_version, Some(&why));
                Ok(ServiceMigration::new(
                    "rolled-back",
                    format!("{why}. The old host service runs as before."),
                    false,
                ))
            }
        }
    }

    /// The relay's view of this machine's session: `(online, connected_at)`
    /// (relay mode; `None` when it could not be read). Read fresh, never
    /// the remembered answer [`Host::status`] shows during a hiccup.
    async fn relay_session(&self, config: &HostConfig) -> Option<(bool, Option<u64>)> {
        let (Some(url), Some(id)) = (&config.relay_url, &config.machine_id) else {
            return None;
        };
        if config.mode != "relay" || config.paused_signed_out {
            return None;
        }
        self.relay_machine(url, id)
            .await
            .ok()
            .map(|m| (m.online, m.connected_at))
    }

    /// The helper keeps running, and, when the old service was online at
    /// the relay, a new session of this machine is online there.
    async fn verify_helper(
        &self,
        helper: &dyn ServiceManager,
        config: &HostConfig,
        before: Option<(bool, Option<u64>)>,
    ) -> Result<()> {
        let t = self.verify;
        let start = std::time::Instant::now();
        let mut stable_since: Option<std::time::Instant> = None;
        let was_online = matches!(before, Some((true, _)));
        let mut last = "it did not start".to_string();
        loop {
            if helper.state().running {
                let since = *stable_since.get_or_insert_with(std::time::Instant::now);
                if since.elapsed() >= t.stable {
                    match self.relay_session(config).await {
                        Some((true, at))
                            if !was_online || at.is_none() || at != before.and_then(|b| b.1) =>
                        {
                            return Ok(());
                        }
                        _ if !was_online && start.elapsed() >= t.offline_grace => return Ok(()),
                        Some(_) => last = "it is not online at the relay yet".into(),
                        None if was_online => last = "the relay did not answer".into(),
                        None => {}
                    }
                }
            } else {
                if stable_since.is_some() {
                    last = "it stopped after starting".into();
                }
                stable_since = None;
            }
            if start.elapsed() >= t.timeout {
                return Err(Error::Service(format!(
                    "{last} (checked for {} s)",
                    t.timeout.as_secs()
                )));
            }
            tokio::time::sleep(t.poll).await;
        }
    }

    /// Stops and removes the old LaunchAgent and its binary (the helper
    /// replaced them).
    fn retire_legacy(&self) -> Result<()> {
        let legacy = self.legacy();
        legacy.bootout()?;
        let plist = legacy.plist();
        if plist.is_file() {
            std::fs::remove_file(&plist)?;
        }
        self.remove_legacy_binary();
        Ok(())
    }

    fn remove_legacy_binary(&self) {
        let bin = self.paths.driver_bin();
        let _ = std::fs::remove_file(&bin);
        if let Some(dir) = bin.parent() {
            // Only when empty.
            let _ = std::fs::remove_dir(dir);
        }
    }

    fn record_migration(&self, state: &str, app_version: &str, reason: Option<&str>) {
        let _ = write_json(
            &self.paths.dir.join(helper_migration::RECORD_FILE),
            &MigrationRecord {
                state: state.into(),
                app_version: app_version.into(),
                at_ms: now_ms(),
                reason: reason.map(Into::into),
            },
        );
    }

    async fn remove_inner(&self, uninstalling: bool) -> Result<()> {
        let Some(config) = self.config()? else {
            return Ok(());
        };
        // Spaces it provides would keep running with nobody to delete them.
        let provided = crate::provided::load_provided(&self.paths.dir)?;
        if !provided.is_empty() && !uninstalling {
            return Err(Error::InvalidArgument(format!(
                "this computer still provides {} Space{} ({}); delete {} first (`{} spaces delete <local:name>`)",
                provided.len(),
                if provided.len() == 1 { "" } else { "s" },
                provided
                    .iter()
                    .map(|p| p.local_space.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                if provided.len() == 1 { "it" } else { "them" },
                crate::cli(),
            )));
        }
        let mut relay_error = None;
        let mut note = |e: Error| match e {
            // Already gone: nothing to tell the relay.
            Error::NotFound(_) => {}
            // A refused machine token leaves the entry listed under My
            // machines; report it (after the local cleanup) rather than
            // pretending the unregister worked.
            e => relay_error = Some(e),
        };
        if uninstalling {
            for space in &provided {
                if space.machine_token.is_empty() || space.relay_url.is_empty() {
                    continue;
                }
                if let Err(e) = RelayClient::new(&space.relay_url)?
                    .delete(&space.machine_token, &space.relay_machine)
                    .await
                {
                    note(e);
                }
            }
        }
        if let (Some(url), Some(id)) = (&config.relay_url, &config.machine_id)
            && let Ok(token) = read_secret(&self.paths.machine_token())
            && let Err(e) = RelayClient::new(url)?.delete(&token, id).await
        {
            note(e);
        }
        // Its registrations under other accounts go too: nothing of this
        // computer stays listed anywhere.
        if let Some(url) = &config.relay_url {
            for r in self.registrations().values() {
                if let Err(e) = RelayClient::new(url)?
                    .delete(&r.machine_token, &r.machine_id)
                    .await
                {
                    note(e);
                }
            }
        }
        self.manager_for_config(&config).uninstall()?;
        if config.runner == RunnerKind::AppHelper.as_str() {
            // Whatever an interrupted move left of the old agent.
            let _ = self.retire_legacy();
        }
        remove_dir_retrying(&self.paths.dir).await?;
        match relay_error {
            Some(e) => Err(Error::Relay(format!(
                "removed locally, but the relay could not be told: {e}"
            ))),
            None => Ok(()),
        }
    }
}

/// Renames `from` to `to`, copying across volumes.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}

/// How long [`Host::status`] keeps the relay's last answer about this
/// machine while the relay is briefly unreachable, before it shows the
/// failure ("Needs attention"). A relay deploy replaces its one pod, and
/// the proxy in front of it answers 503 for about 20 s meanwhile; the
/// host service reconnects on its own.
pub const RELAY_HICCUP_GRACE: std::time::Duration = std::time::Duration::from_secs(120);

/// The relay's last answer about each machine (relay URL, machine id),
/// and when it came.
type RelayViews = std::collections::HashMap<(String, String), (std::time::Instant, crate::Machine)>;
static RELAY_VIEWS: std::sync::Mutex<Option<RelayViews>> = std::sync::Mutex::new(None);

/// `read` (the relay's answer about machine `id`), remembered when it
/// worked; a transient failure ([`Error::is_transient`]) within
/// [`RELAY_HICCUP_GRACE`] of the last answer gets that answer instead.
fn recall_relay_view(url: &str, id: &str, read: Result<crate::Machine>) -> Result<crate::Machine> {
    let key = (url.to_string(), id.to_string());
    let mut views = RELAY_VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let views = views.get_or_insert_with(Default::default);
    match read {
        Ok(m) => {
            views.insert(key, (std::time::Instant::now(), m.clone()));
            Ok(m)
        }
        Err(e) if e.is_transient() => match views.get(&key) {
            Some((at, m)) if at.elapsed() < RELAY_HICCUP_GRACE => {
                tracing::debug!("relay status read failed; showing its last answer: {e}");
                Ok(m.clone())
            }
            _ => Err(e),
        },
        Err(e) => Err(e),
    }
}

/// `account` (an id, or an email) is the policy's owner. An empty
/// `account` or owner says nothing, and is taken as the owner.
pub fn owned_by(policy: &HostPolicy, account: &str) -> bool {
    let account = account.trim();
    if account.is_empty() || policy.owner.is_empty() {
        return true;
    }
    policy.owner == account
        || policy
            .owner_email
            .as_deref()
            .is_some_and(|e| e.eq_ignore_ascii_case(account))
}

/// Shared libraries `bin` needs and the system lacks (Linux, via `ldd`);
/// empty where that cannot be checked.
fn missing_libraries(bin: &Path) -> Vec<String> {
    if !cfg!(target_os = "linux") {
        return vec![];
    }
    match std::process::Command::new("ldd").arg(bin).output() {
        Ok(o) => parse_missing_libraries(&String::from_utf8_lossy(&o.stdout)),
        Err(_) => vec![],
    }
}

/// The `name => not found` lines of `ldd` output.
fn parse_missing_libraries(ldd: &str) -> Vec<String> {
    ldd.lines()
        .filter(|l| l.contains("not found"))
        .filter_map(|l| l.split("=>").next())
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

fn missing_libraries_message(missing: &[String]) -> String {
    let hint = if missing
        .iter()
        .any(|m| m.starts_with("libX") || m.starts_with("libxcb"))
    {
        " On Debian or Ubuntu: sudo apt install libxi6 libxtst6 libx11-6 libxcb1 (the host service links the X libraries even without a desktop)."
    } else {
        ""
    };
    format!(
        "cua-spacesd cannot start: this machine lacks {}.{hint} Install them and run `{} host setup` again.",
        missing.join(", "),
        crate::cli()
    )
}

/// After starting the service: it must still be running a few seconds
/// later (a service that exits at once is restarted by its runner and looks
/// "activating"), or setup fails with the reason instead of reporting
/// success.
async fn confirm_running(manager: &dyn crate::service::ServiceManager, log: &Path) -> Result<()> {
    if manager.state().kind == "fake" {
        return Ok(());
    }
    let settle = std::env::var("CUA_HOST_START_SETTLE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(4000);
    let step = std::time::Duration::from_millis((settle / 3).max(1));
    for _ in 0..3 {
        tokio::time::sleep(step).await;
        let st = manager.state();
        if !st.running {
            let tail = std::fs::read_to_string(log)
                .map(|t| {
                    let lines: Vec<&str> = t.lines().rev().take(3).collect();
                    lines.into_iter().rev().collect::<Vec<_>>().join(" | ")
                })
                .unwrap_or_default();
            return Err(Error::Service(format!(
                "the host service did not stay running ({}); last log lines: {}",
                st.detail.trim(),
                if tail.is_empty() { "none" } else { &tail }
            )));
        }
    }
    Ok(())
}

/// Locates (or downloads) cua-spacesd and installs it at `dest`.
async fn install_driver(explicit: Option<&Path>, dest: &Path) -> Result<()> {
    let source = driver::locate(explicit)?;
    match &source {
        DriverSource::Download(url) => tracing::info!(%url, "downloading cua-spacesd"),
        DriverSource::Release { version, url, .. } => {
            tracing::info!(%version, %url, "downloading cua-spacesd")
        }
        DriverSource::Local(_) => {}
    }
    driver::install_to(&source, dest).await
}

/// `remove_dir_all`, retried for up to 5 s: Windows releases a stopped
/// driver's files (its exe, driver.log) a moment after it exits (CUA-1749).
async fn remove_dir_retrying(dir: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) if tries >= 20 => return Err(e),
            Err(_) => {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_libraries_are_read_from_ldd() {
        let out = "\tlinux-vdso.so.1 (0x00007ffd)\n\tlibXi.so.6 => not found\n\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x1)\n\tlibXtst.so.6 => not found\n";
        assert_eq!(
            super::parse_missing_libraries(out),
            vec!["libXi.so.6", "libXtst.so.6"]
        );
        let m = super::missing_libraries_message(&["libXi.so.6".into()]);
        assert!(m.contains("libXi.so.6") && m.contains("apt install"));
        assert!(super::parse_missing_libraries("\tlibc.so.6 => /lib/libc.so.6\n").is_empty());
    }

    #[test]
    fn a_host_registers_its_os_and_arch() {
        // A pool host's cloud, as its cloud-init names it; anything else is
        // left out.
        assert_eq!(
            super::cloud_word(Some(" Hetzner ".into())).as_deref(),
            Some("hetzner")
        );
        assert_eq!(super::cloud_word(Some("aws; rm".into())), None);
        assert_eq!(super::cloud_word(Some(String::new())), None);
        assert_eq!(super::cloud_word(None), None);
        let meta = super::host_meta(true);
        assert_eq!(meta[super::META_PROVIDES_SPACES], "on");
        #[cfg(target_os = "macos")]
        assert_eq!(meta[super::META_OS], "macos");
        #[cfg(target_os = "linux")]
        assert_eq!(meta[super::META_OS], "linux");
        #[cfg(target_arch = "aarch64")]
        assert_eq!(meta[super::META_ARCH], "arm64");
        #[cfg(target_arch = "x86_64")]
        assert_eq!(meta[super::META_ARCH], "amd64");
    }

    #[test]
    fn strip_local_suffix_drops_mdns_and_localdomain() {
        use super::strip_local_suffix;
        assert_eq!(strip_local_suffix("Mac.localdomain"), "Mac");
        assert_eq!(
            strip_local_suffix("Dillons-MacBook-Pro-2.local"),
            "Dillons-MacBook-Pro-2"
        );
        assert_eq!(strip_local_suffix("box.LOCAL"), "box");
        assert_eq!(strip_local_suffix("build.example.com"), "build.example.com");
        assert_eq!(strip_local_suffix(".local"), ".local");
        assert_eq!(strip_local_suffix("  ci  "), "ci");
    }

    use super::*;

    #[test]
    fn machine_ids_match_the_relay_rule() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spacesd/id");
        let id = load_or_create_machine_id(&path).unwrap();
        assert!(valid_machine_id(&id), "{id}");
        assert_eq!(load_or_create_machine_id(&path).unwrap(), id);
        std::fs::write(&path, "NOT VALID").unwrap();
        assert_ne!(load_or_create_machine_id(&path).unwrap(), "NOT VALID");
    }

    #[test]
    fn local_network_is_asked_of_macs_that_provide_spaces() {
        let d = Path::new("/Users/me/.cua/host/bin/cua-spacesd");
        assert!(local_network_hint("linux", true, d).is_none());
        assert!(local_network_hint("macos", false, d).is_none());
        let h = local_network_hint("macos", true, d).unwrap();
        assert_eq!(h.id, "local-network");
        assert!(h.settings_url.ends_with("Privacy_LocalNetwork"));
        assert!(
            h.instructions
                .contains("cua-spacesd (/Users/me/.cua/host/bin/cua-spacesd)")
        );
        assert!(
            h.instructions
                .contains("Privacy & Security → Local Network")
        );
        assert!(h.instructions.contains("`cua daemon stop`"));
    }

    #[test]
    fn permission_hints_name_the_app_helper() {
        let helper = Path::new("/Applications/Cua Spaces.app/Contents/MacOS/CuaSpacesMac");
        for h in permission_hints("macos", helper) {
            assert!(
                h.instructions.contains("turn on Cua Spaces,"),
                "{}",
                h.instructions
            );
            assert!(
                !h.instructions.contains("cua-spacesd"),
                "{}",
                h.instructions
            );
        }
        let ln = local_network_hint("macos", true, helper).unwrap();
        assert!(
            ln.instructions.contains("Allow Cua Spaces to find devices"),
            "{}",
            ln.instructions
        );
        let old = Path::new("/Users/ada/.cua/host/bin/cua-spacesd");
        for h in permission_hints("macos", old) {
            assert!(
                h.instructions
                    .contains("cua-spacesd (/Users/ada/.cua/host/bin/cua-spacesd)"),
                "{}",
                h.instructions
            );
        }
    }

    #[test]
    fn permission_hints_only_on_macos() {
        assert!(permission_hints("linux", Path::new("/x")).is_empty());
        let mac = permission_hints("macos", Path::new("/x/cua-spacesd"));
        assert_eq!(mac.len(), 2);
        assert!(mac[0].settings_url.contains("Privacy_ScreenCapture"));
        assert!(mac[1].settings_url.contains("Privacy_Accessibility"));
    }

    #[test]
    fn direct_urls() {
        assert_eq!(direct_url("10.0.0.5:3211").unwrap(), "http://10.0.0.5:3211");
        assert_eq!(direct_url("[::1]:3211").unwrap(), "http://[::1]:3211");
        assert!(direct_url("0.0.0.0:3211").unwrap().ends_with(":3211"));
        assert!(direct_url("nope").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn secrets_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/secret");
        write_secret(&p, "s").unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(read_secret(&p).unwrap(), "s");
    }

    #[cfg(unix)]
    #[test]
    fn secret_writes_replace_atomically_and_never_follow_a_planted_temp() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("machine-token");
        // The old fixed temp name, pre-created as a symlink and as a
        // world-readable file, must not be written through.
        let outside = dir.path().join("outside");
        std::fs::write(&outside, "untouched").unwrap();
        std::os::unix::fs::symlink(&outside, p.with_extension("tmp")).unwrap();
        write_secret(&p, "first").unwrap();
        write_secret(&p, "second").unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched");
        assert_eq!(read_secret(&p).unwrap(), "second");
        assert!(
            !std::fs::symlink_metadata(&p)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // No temp files are left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp") && n.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn machine_id_is_written_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x/machine-id");
        let id = load_or_create_machine_id(&p).unwrap();
        assert!(valid_machine_id(&id));
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[tokio::test]
    async fn remove_dir_retrying_removes_and_tolerates_missing() {
        let dir = tempfile::tempdir().unwrap();
        let host = dir.path().join("host");
        std::fs::create_dir_all(host.join("bin")).unwrap();
        std::fs::write(host.join("driver.log"), "x").unwrap();
        super::remove_dir_retrying(&host).await.unwrap();
        assert!(!host.exists());
        // Already gone (removed by another step): not an error.
        super::remove_dir_retrying(&host).await.unwrap();
    }
}
