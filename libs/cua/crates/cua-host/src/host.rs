//! [`Host`]: setup / status / stop / start / remove.

use crate::driver::{self, DriverSource};
use crate::preflight::{self, SessionProbe};
use crate::relay::{AccountTokens, ConnectedClient, MachinePatch, RegisterRequest, RelayClient};
use crate::service::{self, RunnerKind, ServiceManager, ServiceSpec, ServiceState};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
    /// Machine id (relay mode).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
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
    let d = driver.display();
    vec![
        PermissionHint {
            id: "screen-recording".into(),
            title: "Screen Recording".into(),
            settings_url:
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
                    .into(),
            instructions: format!(
                "System Settings → Privacy & Security → Screen & System Audio Recording: turn on cua-spacesd ({d}), so people you share with can see this screen."
            ),
        },
        PermissionHint {
            id: "accessibility".into(),
            title: "Accessibility".into(),
            settings_url:
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
                    .into(),
            instructions: format!(
                "System Settings → Privacy & Security → Accessibility: turn on cua-spacesd ({d}), so they can control the mouse and keyboard."
            ),
        },
    ]
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
        .unwrap_or_else(|| "this machine".into())
}

/// The LAN address a direct client would use for an unspecified bind.
fn lan_ip() -> Option<std::net::IpAddr> {
    // No packet is sent: connecting a UDP socket only picks a route.
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

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
        ServiceSpec {
            program: config.driver_bin.clone(),
            args,
            env: vec![("CUA_ENV_LOG".into(), "info".into())],
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
        let runner = match &self.manager {
            Some(m) => m.kind(),
            None => opts.runner.resolve(),
        };
        let name = opts
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(hostname);
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
        if runner == RunnerKind::Launchd {
            preflight::check(self.preflight_probe().as_ref(), &self.paths.driver_bin())?;
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
            driver_bin: self.paths.driver_bin(),
            sharing: true,
            share_desktop: opts.share_desktop,
            provide_spaces: opts.provide_spaces,
            ..Default::default()
        };
        match &opts.mode {
            HostMode::Relay { url } => {
                let relay = RelayClient::new(url)?;
                let token = tokens.access_token().await?;
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
                            meta: Default::default(),
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
                if let Some(m) = opts.max_spaces {
                    policy.max_spaces = m;
                }
                write_json(&self.paths.policy(), &policy)?;
            }
        }
        let source = driver::locate(opts.driver_bin.as_deref())?;
        if let DriverSource::Download(url) = &source {
            tracing::info!(%url, "downloading cua-spacesd");
        }
        driver::install_to(&source, &config.driver_bin).await?;
        write_json(&self.paths.config(), &config)?;
        let manager = self.manager(runner);
        manager.install(&self.service_spec(&config))?;
        manager.start()?;
        let _ = crate::provided::audit(
            &self.paths.dir,
            "config",
            "local",
            &config.name,
            &settings_detail(config.share_desktop, config.provide_spaces),
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
        if !desktop && !provide {
            return Err(Error::InvalidArgument(
                "with neither the desktop nor Spaces there is nothing to share; use `cua host stop` to pause, or turn one on"
                    .into(),
            ));
        }
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
        let restart =
            desktop != config.share_desktop || (!relay && provide != config.provide_spaces);
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
            permissions: permission_hints(std::env::consts::OS, &config.driver_bin),
            ..Default::default()
        };
        if config.mode == "direct" {
            status.sharing = config.sharing && status.service.running;
        }
        if let (Some(url), Some(id)) = (&config.relay_url, &config.machine_id) {
            match self.relay_machine(url, id).await {
                Ok(m) => {
                    status.online = Some(m.online);
                    status.sharing = m.sharing;
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

    /// Fills the recent accesses from the driver's log and adds the callers
    /// active right now to `clients`: every one in direct mode (there is no
    /// relay presence), and in relay mode the ones the relay cannot see
    /// (token and viewer callers) or all of them when the relay did not
    /// answer.
    fn add_access(&self, status: &mut HostStatus, now_ms: u64) {
        let report = crate::access::read(&self.paths.access_log(), crate::access::RECENT_LIMIT);
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

    fn require_config(&self) -> Result<HostConfig> {
        self.config()?.ok_or_else(|| {
            Error::NotFound("this machine is not set up for access (run `cua host setup`)".into())
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

    /// Undoes [`Host::stop_sharing`].
    pub async fn start_sharing(&self) -> Result<HostStatus> {
        let mut config = self.require_config()?;
        match (&config.relay_url, &config.machine_id) {
            (Some(url), Some(id)) => {
                let token = read_secret(&self.paths.machine_token())?;
                RelayClient::new(url)?.start_sharing(&token, id).await?;
                self.set_policy_sharing(true)?;
            }
            _ => {
                self.manager_for_config(&config).start()?;
            }
        }
        config.sharing = true;
        write_json(&self.paths.config(), &config)?;
        self.status().await
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
                "name and allowlist live on the relay; this machine is in direct mode".into(),
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
        let Some(config) = self.config()? else {
            return Ok(());
        };
        // Spaces it provides would keep running with nobody to delete them.
        let provided = crate::provided::load_provided(&self.paths.dir)?;
        if !provided.is_empty() {
            return Err(Error::InvalidArgument(format!(
                "this machine still provides {} Space{} ({}); delete {} first (`cua spaces delete <local:name>`)",
                provided.len(),
                if provided.len() == 1 { "" } else { "s" },
                provided
                    .iter()
                    .map(|p| p.local_space.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                if provided.len() == 1 { "it" } else { "them" },
            )));
        }
        let mut relay_error = None;
        if let (Some(url), Some(id)) = (&config.relay_url, &config.machine_id)
            && let Ok(token) = read_secret(&self.paths.machine_token())
        {
            match RelayClient::new(url)?.delete(&token, id).await {
                // Already gone: nothing to tell the relay.
                Ok(()) | Err(Error::NotFound(_)) => {}
                // A refused machine token leaves the entry listed under My
                // machines; report it (after the local cleanup) rather than
                // pretending the unregister worked.
                Err(e) => relay_error = Some(e),
            }
        }
        self.manager_for_config(&config).uninstall()?;
        std::fs::remove_dir_all(&self.paths.dir)?;
        match relay_error {
            Some(e) => Err(Error::Relay(format!(
                "removed locally, but the relay could not be told: {e}"
            ))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
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
}
