// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The app's command layer over the cua SDK, free of Tauri.
//!
//! Every Tauri command in [`crate::commands`] is a one-line call into
//! [`AppCore`], so the whole surface is testable (and drivable from an
//! integration harness) without a window, a webview or an `AppHandle`.
//!
//! - Spaces come from `cua-spaces` in-process: the registry lives in
//!   `~/.cua/spaces.json` (shared with `cua daemon`, the `cua` CLI and
//!   `cua daemon mcp`), and the primitives (files, streams, teleport,
//!   hotspot, agents) are SDK calls. No ssh, no rcdp CLI, no Python.
//! - Fleet is one `cua_fleet::FleetClient` whose bearer comes from the
//!   signed-in user when there is one and from the environment
//!   credentials otherwise ([`AppTokens`]).
//! - `cua daemon` is connected (or started) for what a webview cannot do by
//!   itself: attaching to a Fleet Space's media socket, which needs gateway
//!   headers. The daemon's `OpenMediaBridge` hands back a loopback ticket
//!   URL instead. Direct and local Spaces need no bridge: their media
//!   tickets are safe in a URL.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use base64::Engine;
use cua_daemon::client::{DaemonAddress, DaemonClient};
use cua_fleet::sdk::{AccessTokenProvider, AccessTokenProviderError};
use cua_fleet::{FleetClient, FleetConfig};
use cua_sandbox_core::placement::{Kind, On, Runtime};
use cua_sandbox_core::settings::{Settings, Source};
use cua_sandbox_core::LocalRuntime;
use cua_spaces::files::SendFileOptions;
use cua_spaces::hotspot::HotspotOptions;
use cua_spaces::operator::OperatorDisplay;
use cua_spaces::{Provider, Space, SpaceCreate, SpaceInfo, Spaces};
use cua_spaces_ext::teleport::{
    AppSessions, ImportOptions, SpaceTeleport as _, TeleportManifest, TeleportScope,
};
use cua_spaces_ext::teleport_app::{SpaceAppTeleport as _, SpaceAppTeleportFacts as _};
use cua_spacesd_client::pb;
use serde::{Deserialize, Serialize};

use crate::auth::{Credentials, SessionHandle, Store, TokenError};

/// Result type of every command: the error is the user-facing message.
pub type CmdResult<T> = Result<T, String>;

fn msg(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// POSIX single-quoting.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The guest script that opens `cli` in an xterm on the Space's desktop,
/// with the cua tool bins first on PATH.
fn agent_terminal_script(title: &str, cli: &str) -> String {
    let inner = format!("PATH=\"$HOME/.cua/bin:$PATH\"; export PATH; exec {cli}");
    format!(
        "export DISPLAY=\"${{DISPLAY:-:1}}\"; exec xterm -T {} -e sh -c {}",
        sh_quote(title),
        sh_quote(&inner)
    )
}

/// Bounded wait for one Space's handshake while listing.
const LIST_PROBE: Duration = Duration::from_secs(3);
/// How long `cua daemon start` may take.
const DAEMON_START_BUDGET: Duration = Duration::from_secs(20);

// ---------------------------------------------------------------- config

/// How the app reaches `cua daemon`.
#[derive(Clone, Debug)]
pub enum DaemonMode {
    /// Never talk to a daemon (tests; Fleet streams then fail clearly).
    Disabled,
    /// Connect through `<home>/daemon.json`; when nothing answers and
    /// `cua_bin` is set, run `<cua_bin> daemon start` once.
    Auto {
        /// The `cua` binary used to start the daemon.
        cua_bin: Option<PathBuf>,
    },
}

/// Everything [`AppCore`] is built from. [`CoreConfig::from_env`] is the
/// app; tests replace the pieces that would touch the host.
pub struct CoreConfig {
    /// `~/.cua` (or `$CUA_HOME`): registry, daemon discovery, control file.
    pub home: PathBuf,
    /// Fleet endpoint and environment credentials.
    pub fleet: FleetConfig,
    /// A ready Fleet client (tests: `FakeFleet`). Replaces [`AppTokens`].
    pub fleet_client: Option<FleetClient>,
    /// Fleet namespace for claimed Spaces; `None` derives one.
    pub fleet_namespace: Option<String>,
    /// Where the signed-in user's session is persisted.
    pub token_store: Store,
    /// Local runtime (`cua-vmm`: containers, gVisor, QEMU, Lume).
    pub local_runtime: Option<Arc<dyn LocalRuntime>>,
    /// sandbox-core state directory; `None` shares the SDK default with
    /// the daemon and the CLI.
    pub state_dir: Option<PathBuf>,
    /// Teleport senders (real host in the app; `FakeHost` in tests).
    pub app_sessions: Arc<AppSessions>,
    /// Presenting Spaces on this desktop (the app's own windows).
    pub operator_display: Option<Arc<dyn OperatorDisplay>>,
    /// Where `download` lands.
    pub download_dir: Option<PathBuf>,
    /// spacesd handshake budget.
    pub probe_timeout: Duration,
    /// `cua daemon`.
    pub daemon: DaemonMode,
    /// Image for "Local" container Spaces.
    pub local_container_image: String,
    /// Image for "Local" macOS Spaces (Lume), when configured.
    pub local_macos_image: Option<String>,
    /// The cua.ai relay machines of the signed-in account appear in the
    /// roster as `relay:<id>`. `None`: the default relay
    /// (`CUA_RELAY_URL` or https://relay.cua.ai) with the app's session,
    /// once signed in. Tests pass a relay and tokens explicitly.
    pub relay: Option<cua_spaces::RelayAccount>,
    /// "Teleport an app…" app roots; `None` scans this machine's
    /// (`/Applications`, `~/Applications`). Tests pass fixture directories.
    pub teleport_app_roots: Option<Vec<PathBuf>>,
    /// Teleport recents file; `None` is `~/.cua/teleport-recents.json`.
    pub teleport_recents: Option<PathBuf>,
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// `$CUA_HOME` or `~/.cua`.
pub fn cua_home() -> PathBuf {
    cua_daemon::cua_home()
}

/// The `cua` binary: `$CUA_BIN`, a `cua` next to the app executable
/// (bundled sidecar), then `cua` on `PATH`.
pub fn find_cua_bin() -> Option<PathBuf> {
    if let Some(p) = non_empty_env("CUA_BIN").map(PathBuf::from) {
        if p.is_file() {
            return Some(p);
        }
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        let sidecar = dir.join("cua");
        if sidecar.is_file() {
            return Some(sidecar);
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("cua"))
        .find(|p| p.is_file())
}

impl CoreConfig {
    /// The app's configuration: real host, environment credentials.
    pub fn from_env() -> Self {
        let home = cua_home();
        Self {
            fleet: FleetConfig::from_env(),
            fleet_client: None,
            fleet_namespace: non_empty_env("CUA_SPACES_NAMESPACE"),
            // The credential store shared with the cua CLI and daemon; the
            // app's former session file moves into it.
            token_store: {
                let store = Store::from_env();
                if let Err(e) = store.migrate_legacy(&home) {
                    tracing::warn!("could not migrate the old Spaces session: {e}");
                }
                store
            },
            local_runtime: Some(Arc::new(cua_daemon::local::VmmLocal::default())),
            state_dir: None,
            app_sessions: Arc::new(AppSessions::builtin()),
            operator_display: None,
            download_dir: None,
            probe_timeout: Duration::from_secs(15),
            daemon: DaemonMode::Auto {
                cua_bin: find_cua_bin(),
            },
            // The canonical Linux image (`CUA_IMAGE_LINUX` overrides it); kind
            // `auto` picks its container rootfs variant.
            local_container_image: non_empty_env("CUA_SPACES_LOCAL_IMAGE")
                .unwrap_or_else(|| cua_fleet::canonical_image("linux")),
            local_macos_image: non_empty_env("CUA_SPACES_MACOS_IMAGE")
                .or_else(|| non_empty_env("CUA_LUME_GOLDEN_IMAGE"))
                .map(|i| {
                    if i.starts_with("lume:") {
                        i
                    } else {
                        format!("lume:{i}")
                    }
                }),
            relay: None,
            teleport_app_roots: None,
            teleport_recents: Some(home.join("teleport-recents.json")),
            home,
        }
    }

    /// A hermetic configuration rooted at `home`: no Fleet credentials, no
    /// local runtime, no daemon, fake teleport host. Tests adjust from here.
    pub fn hermetic(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
            fleet: FleetConfig::default(),
            fleet_client: None,
            fleet_namespace: Some("cua-e2e-app".into()),
            token_store: Store::File(home.join("no-user-session.json")),
            local_runtime: None,
            state_dir: Some(home.join("sandboxes")),
            app_sessions: Arc::new(AppSessions::with_host(Arc::new(
                cua_spaces_ext::teleport::providers::FakeHost::new().with_home(home),
            ))),
            operator_display: Some(Arc::new(cua_spaces::operator::NoDisplay)),
            download_dir: Some(home.join("downloads")),
            probe_timeout: Duration::from_secs(10),
            daemon: DaemonMode::Disabled,
            relay: None,
            local_container_image: "cua-e2e-local/linux:docker-local-arm64".into(),
            local_macos_image: None,
            teleport_app_roots: Some(vec![home.join("Applications")]),
            teleport_recents: Some(home.join("teleport-recents.json")),
        }
    }
}

// ----------------------------------------------------------- Fleet tokens

/// Fleet bearer source: the signed-in user's token (refreshed as needed),
/// else the environment credentials, else a clear "sign in" error.
pub struct AppTokens {
    session: Arc<SessionHandle>,
    signed_in: Arc<AtomicBool>,
    env: Option<FleetClient>,
}

#[async_trait::async_trait]
impl AccessTokenProvider for AppTokens {
    async fn get_access_token(&self, force: bool) -> Result<String, AccessTokenProviderError> {
        if self.signed_in.load(Ordering::Relaxed) {
            match self.session.get_valid_token(force).await {
                Ok(token) => return Ok(token),
                Err(TokenError::NoSession) => self.signed_in.store(false, Ordering::Relaxed),
                Err(TokenError::Refresh(reason)) => {
                    tracing::warn!("user session refresh failed, falling back: {reason}");
                    self.signed_in.store(false, Ordering::Relaxed);
                }
            }
        }
        match &self.env {
            Some(client) => {
                client
                    .access_token(force)
                    .await
                    .map_err(|e| AccessTokenProviderError::Failed {
                        reason: e.to_string(),
                    })
            }
            None => Err(AccessTokenProviderError::Failed {
                reason: "Cua Cloud is not configured: sign in to Cua (or run `cua auth login`), \
                         or set CUA_CLIENT_ID and CUA_CLIENT_SECRET (or FLEETS_TOKEN)"
                    .into(),
            }),
        }
    }
}

// ------------------------------------------------------------- wire types

/// How the Fleet bearer is obtained (Settings shows it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    /// A signed-in user's device-grant token.
    User,
    /// `CUA_CLIENT_ID` + `CUA_CLIENT_SECRET`.
    ClientCredentials,
    /// `FLEETS_TOKEN`.
    StaticToken,
    /// Nothing.
    None,
}

/// `fleet_status`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetStatus {
    pub configured: bool,
    pub auth_mode: AuthMode,
    pub base_url: String,
    pub token_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespaces: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_error: Option<String>,
}

/// `daemon_status` / `ensure_daemon`.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub socket_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loopback_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `local_status`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStatus {
    pub available: bool,
    pub backends: Vec<String>,
    pub container_image: String,
    pub macos_image: Option<String>,
    pub error: Option<String>,
    /// This Mac's architecture (`arm64`, `amd64`).
    pub host_arch: String,
    /// Free space where local Spaces are written and what is pulled (the
    /// New Space wizard's Resources step).
    pub storage: Option<cua_spaces_app_core::wizard::LocalStorage>,
}

/// The SDK's storage probe as the wizard env takes it.
fn wizard_storage(
    r: cua_daemon::storage::StorageReport,
) -> cua_spaces_app_core::wizard::LocalStorage {
    let vol = |v: Option<cua_daemon::storage::Volume>| {
        v.map(|v| cua_spaces_app_core::wizard::StorageVolume {
            available_bytes: v.available,
            total_bytes: v.total,
            name: v.name,
        })
    };
    cua_spaces_app_core::wizard::LocalStorage {
        reserve_bytes: r.reserve,
        lume: vol(r.lume),
        qemu: vol(r.qemu),
        container: vol(r.container),
        pulled: r.pulled,
    }
}

/// `arm64` / `amd64`, as the image catalog spells them (the app core's).
pub fn host_arch() -> String {
    cua_spaces_app_core::model::this_host_arch().into()
}

/// One registered Space as the switcher shows it.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SpaceRow {
    pub id: String,
    pub name: String,
    pub provider: Provider,
    pub spacesd_version: String,
    pub features: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// OS product or distribution ("Ubuntu"), for the OS icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_name: Option<String>,
    /// The full OS string ("Ubuntu 24.04.3 LTS").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os_pretty_name: Option<String>,
    /// The image it runs and its digest, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_digest: Option<String>,
    /// `container` or `vm`, and the guest's CPU architecture, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    pub reachable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// For a Space one of your machines provides: that machine's relay id
    /// and name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
    /// How it turns off and on (`suspend`, `stop`), and how cua last left
    /// it (`running`, `suspended`, `stopped`); absent when it cannot or
    /// unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub power_state: Option<String>,
    /// For a Space in your cloud: the provider word, its account and
    /// region in words, and where Delete permanently can delete it
    /// (`here`, `host:<machine>`, `elsewhere`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud_place: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud_delete: Option<String>,
}

/// The name to show for a registered Space: the app core's rule (a
/// container's 12-hex hostname defers to the name in the id).
pub fn display_name(id: &str, reported: &str) -> String {
    cua_spaces_app_core::spaces::name_of(id, reported)
}

impl SpaceRow {
    fn from_info(info: SpaceInfo) -> Self {
        let name = display_name(&info.id, &info.name);
        Self {
            id: info.id,
            name,
            provider: info.provider,
            spacesd_version: info.spacesd_version,
            features: info.features,
            os_name: (!info.os_name.is_empty()).then_some(info.os_name),
            os_pretty_name: (!info.os_pretty_name.is_empty()).then_some(info.os_pretty_name),
            image: (!info.image.is_empty()).then_some(info.image),
            image_digest: (!info.image_digest.is_empty()).then_some(info.image_digest),
            kind: (!info.kind.is_empty()).then_some(info.kind),
            arch: (!info.arch.is_empty()).then_some(info.arch),
            added_at: info.added_at,
            os: None,
            reachable: false,
            error: None,
            host: (!info.host.is_empty()).then_some(info.host),
            host_name: (!info.host_name.is_empty()).then_some(info.host_name),
            power: (!info.power.is_empty()).then_some(info.power),
            power_state: (!info.power_state.is_empty()).then_some(info.power_state),
            cloud: (!info.cloud.is_empty()).then_some(info.cloud),
            cloud_place: (!info.cloud_place.is_empty()).then_some(info.cloud_place),
            cloud_delete: (!info.cloud_delete.is_empty()).then_some(info.cloud_delete),
        }
    }

    fn connected(info: SpaceInfo, space: &Space) -> Self {
        let mut row = Self::from_info(info);
        row.os = Some(os_name(space).into());
        if let Some(name) = space
            .capabilities()
            .os
            .as_ref()
            .map(|o| o.name.clone())
            .filter(|n| !n.is_empty())
        {
            row.os_name = Some(name);
        }
        if let Some(pretty) = space
            .capabilities()
            .os
            .as_ref()
            .map(|o| o.pretty_name.clone())
            .filter(|n| !n.is_empty())
        {
            row.os_pretty_name = Some(pretty);
        }
        if let Some((image, digest)) = space.image() {
            row.image = Some(image.to_string());
            row.image_digest = (!digest.is_empty()).then(|| digest.to_string());
        }
        // The guest's own runtime and arch beat the record.
        let (kind, arch) = space.platform();
        if !kind.is_empty() {
            row.kind = Some(kind);
        }
        if !arch.is_empty() {
            row.arch = Some(arch);
        }
        row.reachable = true;
        // Live capabilities beat the registry snapshot.
        row.features = space
            .capabilities()
            .features
            .iter()
            .filter(|f| f.supported)
            .map(|f| f.name.clone())
            .collect();
        row
    }
}

fn os_name(space: &Space) -> &'static str {
    match space.os_family() {
        pb::OsFamily::Macos => "macos",
        pb::OsFamily::Windows => "windows",
        _ => "linux",
    }
}

/// `create_space` input: where (`on`), what (`kind`) and which engine
/// (`runtime`), as the SDK's placement model spells them. Every field is
/// optional: `on` defaults to the configured default location
/// (`default.on`), `kind` and `runtime` to `auto`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCreateConfig {
    pub image: Option<String>,
    /// `local` or `cloud`.
    pub on: Option<String>,
    /// `auto`, `container` or `vm`.
    pub kind: Option<String>,
    /// `auto`, or an engine the location offers (local: gvisor, runc, qemu,
    /// lume; cloud: gvisor, kubevirt).
    pub runtime: Option<String>,
    pub name: Option<String>,
    /// vCPUs (local and Cua Cloud, within `cua_fleet::FLEET_ABSOLUTE_CPUS`).
    pub cpus: Option<u32>,
    /// Memory MB (local and Cua Cloud, within `cua_fleet::FLEET_ABSOLUTE_MEMORY_MB`).
    pub memory_mb: Option<u64>,
    /// Local VMs only: grow the disk to this many GB.
    pub disk_gb: Option<u32>,
    /// Whether the image runs cua-spacesd (the shared image list says). None:
    /// the SDK decides from the image.
    pub spacesd: Option<bool>,
    /// Return a reachable registered Space in that location instead of
    /// creating one.
    pub reuse: Option<bool>,
    /// A GPU option of the runtime (`paravirtual`, from [`AppCore::gpu_support`]);
    /// `None`: no GPU.
    #[serde(default)]
    pub gpu: Option<String>,
}

/// What a cancelled create's error starts with (the SDK's `cancelled` tag),
/// so the webview tells a cancel from a failure.
pub const CANCELLED_PREFIX: &str = "cancelled: ";

/// A create's error for the webview: the SDK's message, prefixed with
/// [`CANCELLED_PREFIX`] when the create was cancelled.
pub fn create_error(e: &cua_spaces::Error) -> String {
    if e.tag() == "cancelled" {
        format!("{CANCELLED_PREFIX}{e}")
    } else {
        e.to_string()
    }
}

/// The wizard's GPU choices from the SDK's `gpu_support`: the first option
/// of each runtime that has one (a runtime with none gets no GPU row).
pub fn gpu_choices(
    support: &[cua_sandbox_core::gpu::GpuSupport],
) -> Vec<cua_spaces_app_core::wizard::GpuChoice> {
    support
        .iter()
        .filter_map(|s| {
            let o = s.options.first()?;
            Some(cua_spaces_app_core::wizard::GpuChoice {
                runtime: s.runtime.clone(),
                id: o.id.clone(),
                label: o.label.clone(),
                experimental: o.experimental,
                supported: o.supported,
                reason: Some(o.reason.clone()).filter(|r| !r.trim().is_empty()),
                learn_more: o.learn_more.clone(),
            })
        })
        .collect()
}

/// One of your machines that provides Spaces, as the wizard reads it.
pub fn space_host(h: cua_spaces::host_spaces::HostOffer) -> cua_spaces_app_core::wizard::SpaceHost {
    cua_spaces_app_core::wizard::SpaceHost {
        id: h.id,
        name: h.name,
        via: h.via,
        online: h.online,
        os: h.os,
        limits: h
            .limits
            .into_iter()
            .map(|l| cua_spaces_app_core::wizard::HostLimit {
                resource: l.resource,
                used: l.used,
                limit: l.limit,
                reason: l.reason,
            })
            .collect(),
    }
}

/// `get_default_location` / `set_default_location`.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DefaultLocation {
    /// `local` or `cloud` (or a registered provider).
    pub value: String,
    /// `env`, `config` or `default`.
    pub source: String,
    /// The environment variable, when `source` is `env` (the app cannot
    /// change it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// The config file the value is stored in.
    pub path: String,
}

/// Splits a legacy engine prefix (`container:`, `vm:`, `lume:`) off an
/// image reference into the placement it meant.
fn split_image_prefix(image: &str) -> (&str, Option<Kind>, Option<Runtime>) {
    if let Some(rest) = image.strip_prefix("container:") {
        (rest, Some(Kind::Container), None)
    } else if let Some(rest) = image.strip_prefix("vm:") {
        (rest, Some(Kind::Vm), None)
    } else if let Some(rest) = image.strip_prefix("lume:") {
        (rest, Some(Kind::Vm), Some(Runtime::Lume))
    } else {
        (image, None, None)
    }
}

/// What to stream.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum StreamTargetArg {
    /// A display (`None` = primary).
    #[serde(rename_all = "camelCase")]
    Display { display_id: Option<String> },
    /// One window.
    #[serde(rename_all = "camelCase")]
    Window { window_id: String },
}

/// Stream options from the webview.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamOpts {
    pub max_fps: Option<u32>,
    pub max_dimension: Option<u32>,
    pub audio: Option<bool>,
    pub codecs: Option<Vec<String>>,
    /// `view_only`, `background_only` or `allow_activation` (default).
    pub policy: Option<String>,
    pub geometry_control: Option<bool>,
}

/// A media ticket the webview attaches to.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamTicketInfo {
    pub space_id: String,
    pub media_session_id: String,
    pub ws_url: String,
    pub ticket: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticket_expires_at: Option<String>,
    pub codec: String,
    pub wire_version: u32,
    pub frame_size: [u32; 2],
    /// `direct` (spacesd ticket URL) or `daemon` (loopback bridge).
    pub via: &'static str,
    pub audio: bool,
}

/// A remote window (the picker's "This Mac" tab).
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWindow {
    pub id: String,
    pub app_name: String,
    pub title: String,
    pub visible: bool,
    pub app_id: String,
    pub target_epoch: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width_px: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height_px: Option<u32>,
    /// Owning process (the SDK's app icon lookup), when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
}

/// One window's app, for `space_app_icons`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AppIconRequest {
    pub app_name: String,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub pid: u32,
}

/// The primary display's size (the Stream section's "Desktop (W×H)").
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DisplaySize {
    pub width_px: u32,
    pub height_px: u32,
}

/// One file landed in a Space (`send_files_to_space`).
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SentFile {
    pub name: String,
    pub dest: String,
    pub bytes: u64,
    pub sha256: String,
}

/// Consent-sheet item (snake_case: the webview's `ManifestItem`).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ManifestItem {
    pub label: String,
    pub rel_path: String,
    pub est_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count_noun: Option<String>,
    pub sensitive: bool,
    pub default_checked: bool,
}

/// Consent-sheet manifest (the webview's `TransferManifest`).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct TransferManifest {
    pub provider_id: String,
    pub app_display_name: String,
    pub scope: &'static str,
    pub items: Vec<ManifestItem>,
    pub total_est_bytes: u64,
    pub notes: Vec<String>,
    pub supports_hotspot: bool,
}

impl TransferManifest {
    fn from_sdk(m: &TeleportManifest) -> Self {
        Self {
            provider_id: m.app.clone(),
            app_display_name: m.display_name.clone(),
            scope: match m.scope {
                TeleportScope::Full => "full_profile",
                TeleportScope::Tabs => "tabs_only",
            },
            items: m
                .items
                .iter()
                .map(|i| ManifestItem {
                    label: i.label.clone(),
                    rel_path: i.relative_path.clone(),
                    est_bytes: i.estimated_bytes,
                    count: i.count,
                    count_noun: i.count_noun.clone(),
                    sensitive: i.is_sensitive,
                    default_checked: i.is_checked_by_default,
                })
                .collect(),
            total_est_bytes: m.total_estimated_bytes,
            notes: m.notes.clone(),
            // Browsers can route through the hotspot (SOCKS proxy).
            supports_hotspot: matches!(m.app.as_str(), "chrome" | "google-chrome" | "firefox"),
        }
    }
}

/// `teleport_push` result (the webview's `TeleportResult`).
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct TeleportResult {
    pub ok: bool,
    pub provider_id: String,
    pub launched: bool,
    pub pid: Option<u32>,
    pub imported: Vec<String>,
    pub skipped: Vec<String>,
}

/// `hotspot_*`.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HotspotStatus {
    pub active: bool,
    pub space_id: Option<String>,
}

// ------------------------------------------------------------------ core

/// The app's SDK state. Cheap to share (`Arc<AppCore>`).
pub struct AppCore {
    home: PathBuf,
    fleet_config: FleetConfig,
    fleet_client: Option<FleetClient>,
    session: Arc<SessionHandle>,
    signed_in: Arc<AtomicBool>,
    env_auth: bool,
    spaces: RwLock<Spaces>,
    app_sessions: Arc<AppSessions>,
    teleport_app_roots: Option<Vec<PathBuf>>,
    teleport_recents: Option<PathBuf>,
    daemon_mode: DaemonMode,
    daemon: tokio::sync::Mutex<Option<DaemonClient>>,
    hotspot: Mutex<Option<String>>,
    local_container_image: String,
    local_macos_image: Option<String>,
    has_local: bool,
    // Kept for rebuilding `spaces` after sign-in / sign-out.
    rebuild: SpacesParts,
    /// This device on the relay (one per identity, so its cached session is
    /// shared by listing, connecting and the Devices page).
    device: Mutex<Option<Arc<cua_host::DeviceAuth>>>,
}

struct SpacesParts {
    local_runtime: Option<Arc<dyn LocalRuntime>>,
    state_dir: Option<PathBuf>,
    operator_display: Option<Arc<dyn OperatorDisplay>>,
    download_dir: Option<PathBuf>,
    probe_timeout: Duration,
    fleet_namespace: Option<String>,
    relay: Option<cua_spaces::RelayAccount>,
    /// Where this device's relay key lives (the session's vault).
    device_keys: Store,
}

impl AppCore {
    /// Builds the core. No I/O beyond reading the stored user session.
    pub fn new(cfg: CoreConfig) -> Arc<Self> {
        let device_keys = cfg.token_store.clone();
        let (session, restored) = SessionHandle::new(cfg.token_store);
        let signed_in = Arc::new(AtomicBool::new(restored));
        let env_auth = cfg.fleet.has_auth();
        let fleet_client = match cfg.fleet_client {
            Some(c) => Some(c),
            None => {
                let env = if env_auth {
                    FleetClient::connect(cfg.fleet.clone())
                        .map_err(|e| tracing::warn!("Fleet credentials rejected: {e}"))
                        .ok()
                } else {
                    None
                };
                let tokens = Arc::new(AppTokens {
                    session: session.clone(),
                    signed_in: signed_in.clone(),
                    env,
                });
                FleetClient::connect_with_token_provider(cfg.fleet.clone(), tokens, None)
                    .map_err(|e| tracing::warn!("Fleet client: {e}"))
                    .ok()
            }
        };
        let rebuild = SpacesParts {
            local_runtime: cfg.local_runtime,
            state_dir: cfg.state_dir,
            operator_display: cfg.operator_display,
            download_dir: cfg.download_dir,
            probe_timeout: cfg.probe_timeout,
            fleet_namespace: cfg.fleet_namespace,
            relay: cfg.relay,
            device_keys,
        };
        let core = Self {
            has_local: rebuild.local_runtime.is_some(),
            spaces: RwLock::new(Spaces::builder().build()),
            home: cfg.home,
            fleet_config: cfg.fleet,
            fleet_client,
            session,
            signed_in,
            env_auth,
            app_sessions: cfg.app_sessions,
            teleport_app_roots: cfg.teleport_app_roots,
            teleport_recents: cfg.teleport_recents,
            daemon_mode: cfg.daemon,
            daemon: tokio::sync::Mutex::new(None),
            hotspot: Mutex::new(None),
            local_container_image: cfg.local_container_image,
            local_macos_image: cfg.local_macos_image,
            rebuild,
            device: Mutex::new(None),
        };
        let spaces = core.build_spaces(None);
        *core.spaces.write().unwrap_or_else(|p| p.into_inner()) = spaces;
        Arc::new(core)
    }

    fn build_spaces(&self, identity: Option<&str>) -> Spaces {
        let p = &self.rebuild;
        // The Cua Spaces extensions: teleport over this app's providers, the
        // Cua Volume tools and persistent agents over the local drive.
        let mut b = cua_spaces_ext::register(
            Spaces::builder()
                .home(&self.home)
                .probe_timeout(p.probe_timeout),
            cua_volume::Drive::open_local(&self.home),
            Some(self.app_sessions.clone()),
        );
        let mut sandboxes = cua_sandbox_core::Sandboxes::builder();
        if let Some(f) = &self.fleet_client {
            b = b.fleet(f.clone());
            sandboxes = sandboxes.fleet(f.clone());
        }
        if let Some(l) = &p.local_runtime {
            b = b.local_runtime(l.clone());
            sandboxes = sandboxes.local(l.clone());
        }
        if let Some(d) = &p.state_dir {
            sandboxes = sandboxes.state_dir(d);
        }
        b = b.sandboxes(sandboxes.build());
        let ns = p.fleet_namespace.clone().or_else(|| {
            identity
                .map(cua_spaces::sanitize_label)
                .filter(|l| !l.is_empty())
                .map(|l| cua_spaces::sanitize_label(&format!("cua-spaces-{l}")))
        });
        if let Some(ns) = ns {
            b = b.fleet_namespace(ns);
        }
        if let Some(d) = &p.operator_display {
            b = b.operator_display(d.clone());
        } else {
            b = b.operator_display(Arc::new(cua_spaces::operator::ControlServerDisplay::new(
                self.home.join("spaces-control.json"),
            )));
        }
        if let Some(d) = &p.download_dir {
            b = b.download_dir(d);
        }
        // Relay machines of the signed-in account (`relay:<id>`).
        if let Some(relay) = &p.relay {
            b = b.relay(relay.clone());
        } else if self.signed_in.load(Ordering::Relaxed) {
            let url = cua_host::relay_url_from_env();
            let mut account = cua_spaces::RelayAccount::new(url, self.session_tokens());
            // This device's enrollment (key in the session's vault, shared
            // with the cua CLI and daemon): the relay needs its session.
            if let Some(device) = self.device_auth() {
                account = account.with_device(device);
            }
            b = b.relay(account);
        }
        b = b.share_consent(Arc::new(crate::share::AppShareConsent));
        b.build()
    }

    fn session_tokens(&self) -> Arc<dyn cua_host::AccountTokens> {
        Arc::new(crate::host_backend::SessionTokens(self.session.clone()))
    }

    /// This device on the default relay as the signed-in account (its key
    /// in the session's vault), made once per identity.
    fn device_auth(&self) -> Option<Arc<cua_host::DeviceAuth>> {
        let mut slot = self.device.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_none() {
            *slot = cua_host::DeviceAuth::new(
                &cua_host::relay_url_from_env(),
                self.session_tokens(),
                Arc::new(self.rebuild.device_keys.clone()),
                cua_host::device_name(),
            )
            .map(Arc::new)
            .ok();
        }
        slot.clone()
    }

    /// The Devices page's relay calls: the configured relay's device, else
    /// this device on the default relay once signed in.
    pub fn devices(&self) -> Result<crate::devices::DevicesService, String> {
        if let Some(relay) = &self.rebuild.relay {
            return relay
                .device
                .clone()
                .map(|d| crate::devices::DevicesService::new(d, relay.tokens.clone()))
                .ok_or_else(|| "this relay has no device".to_string());
        }
        if !self.signed_in.load(Ordering::Relaxed) {
            return Err("sign in to Cua to see your devices".into());
        }
        let device = self
            .device_auth()
            .ok_or_else(|| "the relay URL is invalid".to_string())?;
        Ok(crate::devices::DevicesService::new(
            device,
            self.session_tokens(),
        ))
    }

    /// The live Spaces handle.
    pub fn spaces(&self) -> Spaces {
        self.spaces
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// `~/.cua` this core uses.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Rebuilds the Spaces handle after the Fleet identity changed (the
    /// namespace follows the signed-in account). Hotspots stop.
    async fn identity_changed(&self) {
        let old = self.spaces();
        let _ = old.hotspot_stop(None).await;
        *self.hotspot.lock().unwrap_or_else(|p| p.into_inner()) = None;
        // A new identity gets a fresh device session.
        *self.device.lock().unwrap_or_else(|p| p.into_inner()) = None;
        let identity = self.session.identity().await;
        let fresh = self.build_spaces(identity.as_deref());
        *self.spaces.write().unwrap_or_else(|p| p.into_inner()) = fresh;
    }

    // --------------------------------------------------------- account

    fn auth_mode(&self) -> AuthMode {
        if self.signed_in.load(Ordering::Relaxed) {
            AuthMode::User
        } else if self.fleet_config.fleet_token.is_some() {
            AuthMode::StaticToken
        } else if self.env_auth {
            AuthMode::ClientCredentials
        } else {
            AuthMode::None
        }
    }

    /// `fleet_status`.
    pub async fn fleet_status(&self, probe: bool) -> FleetStatus {
        let mode = self.auth_mode();
        let identity = if mode == AuthMode::User {
            self.session.identity().await
        } else {
            None
        };
        let mut status = FleetStatus {
            configured: mode != AuthMode::None,
            auth_mode: mode,
            base_url: self.fleet_config.base_url.clone(),
            token_url: self.fleet_config.token_url.clone(),
            client_id: self.fleet_config.client_id.clone(),
            identity,
            namespaces: None,
            probe_error: None,
        };
        if probe && status.configured {
            match &self.fleet_client {
                Some(f) => match f.sdk().list_namespaces().await {
                    Ok(ns) => status.namespaces = Some(ns.into_iter().map(|n| n.name).collect()),
                    Err(e) => status.probe_error = Some(e.to_string()),
                },
                None => status.probe_error = Some("no Fleet client".into()),
            }
        }
        status
    }

    /// The shared user session (device-grant sign-in drives it).
    pub fn session(&self) -> Arc<SessionHandle> {
        self.session.clone()
    }

    /// Installs a freshly approved user session.
    pub async fn install_user_session(&self, c: Credentials) -> CmdResult<Option<String>> {
        let identity = self.session.install(c).await?;
        self.signed_in.store(true, Ordering::Relaxed);
        self.identity_changed().await;
        Ok(identity)
    }

    /// Signs out; environment credentials resume.
    pub async fn sign_out(&self) {
        self.session.clear().await;
        self.signed_in.store(false, Ordering::Relaxed);
        self.identity_changed().await;
    }

    // ---------------------------------------------------------- daemon

    fn discovered_address(&self) -> DaemonAddress {
        match cua_daemon::Discovery::read(&self.home.join("daemon.json")) {
            Some(d) if cfg!(unix) && d.socket_path.is_some() => {
                DaemonAddress::Socket(PathBuf::from(d.socket_path.unwrap_or_default()))
            }
            Some(cua_daemon::Discovery {
                loopback_url: Some(url),
                token: Some(token),
                ..
            }) => DaemonAddress::Url { url, token },
            _ => DaemonAddress::Socket(self.home.join("cua.sock")),
        }
    }

    async fn try_daemon(&self) -> Option<DaemonClient> {
        let client = DaemonClient::new(self.discovered_address()).ok()?;
        tokio::time::timeout(Duration::from_secs(3), client.info())
            .await
            .ok()?
            .ok()?;
        Some(client)
    }

    /// A connected daemon client; starts `cua daemon` when allowed.
    pub async fn daemon(&self, start: bool) -> CmdResult<DaemonClient> {
        let mut slot = self.daemon.lock().await;
        if let Some(c) = slot.as_ref() {
            if tokio::time::timeout(Duration::from_secs(3), c.info())
                .await
                .is_ok_and(|r| r.is_ok())
            {
                return Ok(c.clone());
            }
            *slot = None;
        }
        let cua_bin = match &self.daemon_mode {
            DaemonMode::Disabled => return Err("the cua daemon is disabled in this build".into()),
            DaemonMode::Auto { cua_bin } => cua_bin.clone(),
        };
        if let Some(c) = self.try_daemon().await {
            *slot = Some(c.clone());
            return Ok(c);
        }
        if !start {
            return Err(cua_daemon::DAEMON_NOT_RUNNING.into());
        }
        let bin = cua_bin.ok_or(
            "the cua daemon is not running and no `cua` binary was found (set CUA_BIN or put \
             `cua` on PATH)",
        )?;
        let home = self.home.clone();
        let status = tokio::time::timeout(
            DAEMON_START_BUDGET,
            tokio::process::Command::new(&bin)
                .args(["daemon", "start"])
                .env("CUA_HOME", &home)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .output(),
        )
        .await
        .map_err(|_| format!("`{} daemon start` timed out", bin.display()))?
        .map_err(|e| format!("could not run {}: {e}", bin.display()))?;
        if !status.status.success() {
            return Err(format!(
                "`cua daemon start` failed: {}",
                String::from_utf8_lossy(&status.stderr).trim()
            ));
        }
        let c = self
            .try_daemon()
            .await
            .ok_or("the cua daemon started but does not answer")?;
        *slot = Some(c.clone());
        Ok(c)
    }

    /// `daemon_status` (`start` = `ensure_daemon`).
    pub async fn daemon_status(&self, start: bool) -> DaemonStatus {
        match self.daemon(start).await {
            Ok(c) => match c.info().await {
                Ok(info) => DaemonStatus {
                    connected: true,
                    version: Some(info.version),
                    socket_path: Some(info.socket_path).filter(|s| !s.is_empty()),
                    loopback_url: Some(info.loopback_url).filter(|s| !s.is_empty()),
                    error: None,
                },
                Err(e) => DaemonStatus {
                    error: Some(e.to_string()),
                    ..Default::default()
                },
            },
            Err(e) => DaemonStatus {
                error: Some(e),
                ..Default::default()
            },
        }
    }

    // ----------------------------------------------------------- cloud

    /// `cloud_pricing`: this account's Cua Cloud rates (the SDK caches them
    /// for five minutes). `None` when not signed in or Fleet gave no rates:
    /// the wizard then shows no estimate.
    pub async fn cloud_pricing(&self) -> Option<cua_spaces_app_core::wizard::CloudPricing> {
        let fleet = self.fleet_client.as_ref()?;
        match fleet.usage_pricing().await {
            Ok(p) => p.map(|p| cua_spaces_app_core::wizard::CloudPricing {
                vcpu_hour_usd: p.vcpu_hour_usd,
                memory_gib_hour_usd: p.memory_gib_hour_usd,
            }),
            Err(e) => {
                tracing::debug!("cloud pricing unavailable: {e}");
                None
            }
        }
    }

    /// `billing_status`: the account's Cua Cloud billing as the app core
    /// takes it (Settings' Billing row). `None` without a Fleet client
    /// (not signed in).
    pub async fn billing_status(
        &self,
    ) -> CmdResult<Option<cua_spaces_app_core::billing::BillingStatus>> {
        use cua_spaces_app_core::billing as b;
        let Some(fleet) = self.fleet_client.as_ref() else {
            return Ok(None);
        };
        let s = fleet.billing_status().await.map_err(msg)?;
        Ok(Some(b::BillingStatus {
            billing_enabled: s.billing_enabled,
            card: s.card.map(|c| b::BillingCard {
                brand: c.brand,
                last4: c.last4,
            }),
            credit: s.credit.map(|c| b::BillingCredit {
                balance_usd_cents: c.balance_usd_cents,
            }),
            billing_url: s.billing_url,
        }))
    }

    // ----------------------------------------------------------- local

    /// `local_status`: which local backends are usable (read-only probe).
    pub async fn local_status(&self) -> LocalStatus {
        if !self.has_local {
            return LocalStatus {
                available: false,
                backends: vec![],
                container_image: self.local_container_image.clone(),
                macos_image: self.local_macos_image.clone(),
                error: Some("no local runtime in this build".into()),
                host_arch: host_arch(),
                storage: None,
            };
        }
        let (report, storage) = tokio::join!(
            cua_daemon::local::doctor_report(),
            cua_daemon::storage::storage_report()
        );
        let backends: Vec<String> = report
            .backends
            .iter()
            .filter(|b| b.ready)
            .map(|b| format!("{:?}", b.backend).to_ascii_lowercase())
            .collect();
        LocalStatus {
            available: !backends.is_empty(),
            error: backends
                .is_empty()
                .then(|| "no local runtime is ready (run `cua runtime doctor`)".to_string()),
            backends,
            container_image: self.local_container_image.clone(),
            macos_image: self.local_macos_image.clone(),
            host_arch: host_arch(),
            storage: Some(wizard_storage(storage)),
        }
    }

    // ---------------------------------------------------------- roster

    /// Every registered Space, each probed for at most [`LIST_PROBE`].
    pub async fn list_spaces(&self) -> CmdResult<Vec<SpaceRow>> {
        let spaces = self.spaces();
        let infos = spaces.list_all().await.map_err(msg)?;
        let rows = futures_util::future::join_all(infos.into_iter().map(|info| {
            let spaces = spaces.clone();
            async move {
                match tokio::time::timeout(LIST_PROBE, spaces.space(&info.id)).await {
                    Ok(Ok(space)) => SpaceRow::connected(info, &space),
                    Ok(Err(e)) => SpaceRow {
                        error: Some(e.to_string()),
                        ..SpaceRow::from_info(info)
                    },
                    Err(_) => SpaceRow {
                        error: Some("did not answer within 3 s".into()),
                        ..SpaceRow::from_info(info)
                    },
                }
            }
        }))
        .await;
        Ok(rows)
    }

    /// One registered Space, connected (the viewer reads its features).
    pub async fn space_info(&self, space: &str) -> CmdResult<SpaceRow> {
        let spaces = self.spaces();
        let id = spaces.resolve(space).map_err(msg)?.to_string();
        let info = spaces
            .list()
            .map_err(msg)?
            .into_iter()
            .find(|i| i.id == id)
            .ok_or_else(|| format!("{id} is not registered"))?;
        Ok(self.row(info).await)
    }

    async fn row(&self, info: SpaceInfo) -> SpaceRow {
        match self.spaces().space(&info.id).await {
            Ok(s) => SpaceRow::connected(info, &s),
            Err(e) => SpaceRow {
                error: Some(e.to_string()),
                ..SpaceRow::from_info(info)
            },
        }
    }

    /// "Add Space by address".
    pub async fn add_space(
        &self,
        url: &str,
        token: Option<String>,
        name: Option<String>,
    ) -> CmdResult<SpaceRow> {
        let token = token.filter(|t| !t.trim().is_empty());
        let name = name.filter(|n| !n.trim().is_empty());
        let info = self.spaces().add(url, token, name).await.map_err(msg)?;
        Ok(self.row(info).await)
    }

    /// The settings this core reads: `<home>/config.toml` and the process
    /// environment (`CUA_DEFAULT_ON`, ...).
    fn settings(&self) -> CmdResult<Settings> {
        Settings::load_with(self.home.join("config.toml"), |k| std::env::var(k).ok()).map_err(msg)
    }

    /// Where new Spaces go when the create names no location.
    pub fn default_location(&self) -> CmdResult<DefaultLocation> {
        let settings = self.settings()?;
        let (on, source) = settings.default_on().map_err(msg)?;
        Ok(DefaultLocation {
            value: on.to_string(),
            source: source.kind().into(),
            env: match source {
                Source::Env(v) => Some(v.to_string()),
                _ => None,
            },
            path: settings.path().display().to_string(),
        })
    }

    /// Stores the default location (`local` or `cloud`) in
    /// `<home>/config.toml`, as `cua config set default.on` does. An
    /// environment override still wins; the result says so.
    pub fn set_default_location(&self, on: &str) -> CmdResult<DefaultLocation> {
        let parsed = On::parse(on).map_err(msg)?;
        if parsed.is_existing_machine() {
            return Err(format!(
                "{parsed} names one machine; the default location is local or cloud"
            ));
        }
        let mut settings = self.settings()?;
        let key = cua_sandbox_core::settings::key("default.on").map_err(msg)?;
        settings.set(key, &parsed.to_string()).map_err(msg)?;
        self.default_location()
    }

    /// Creates a Space where `cfg.on` says (the default location when
    /// unset) and blocks until its spacesd answers. Invalid location / kind
    /// / runtime combinations come back as the SDK's placement error, which
    /// lists the valid values.
    pub async fn create_space(&self, cfg: SpaceCreateConfig) -> CmdResult<SpaceRow> {
        self.create_space_with_progress(cfg, None, None).await
    }

    /// [`Self::create_space`], reporting the SDK's create progress (pulling,
    /// booting, waiting for cua-spacesd, connecting) to `progress`.
    /// `create_id` (the webview's pending row id) is what
    /// [`Self::cancel_create`] finds it by; a cancelled create's error
    /// starts with [`CANCELLED_PREFIX`].
    pub async fn create_space_with_progress(
        &self,
        cfg: SpaceCreateConfig,
        progress: Option<cua_spaces::ProgressSink>,
        create_id: Option<String>,
    ) -> CmdResult<SpaceRow> {
        let text = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let on = match text(cfg.on) {
            Some(on) => On::parse(&on).map_err(msg)?,
            None => self.settings()?.default_on().map_err(msg)?.0,
        };
        let mut kind = match text(cfg.kind) {
            Some(k) => Kind::parse(&k).map_err(msg)?,
            None => Kind::Auto,
        };
        let mut runtime = match text(cfg.runtime) {
            Some(r) => Runtime::parse(&r).map_err(msg)?,
            None => Runtime::Auto,
        };
        let image = match (text(cfg.image), &on) {
            (Some(i), _) => Some(i),
            (None, On::Local) if runtime == Runtime::Lume => {
                Some(self.local_macos_image.clone().ok_or(
                    "no macOS Space image is configured (set CUA_SPACES_MACOS_IMAGE to a Lume \
                     image with cua-spacesd, e.g. the golden built by \
                     scripts/build-macos-golden.sh)",
                )?)
            }
            (None, On::Local) if kind != Kind::Vm => Some(self.local_container_image.clone()),
            _ => None,
        };
        let image = image.map(|i| {
            let (bare, k, r) = split_image_prefix(&i);
            if kind == Kind::Auto {
                kind = k.unwrap_or(Kind::Auto);
            }
            if runtime == Runtime::Auto {
                runtime = r.unwrap_or(Runtime::Auto);
            }
            bare.to_string()
        });
        let local = on == On::Local;
        let info = self
            .spaces()
            .create(SpaceCreate {
                on: Some(on),
                image,
                kind,
                runtime,
                name: text(cfg.name),
                cpus: cfg.cpus,
                memory_mb: cfg.memory_mb,
                disk_gb: if local { cfg.disk_gb } else { None },
                wait: Some(true),
                reuse: cfg.reuse.unwrap_or(false),
                spacesd: cfg.spacesd,
                progress,
                create_id: text(create_id),
                gpu: text(cfg.gpu),
                ..Default::default()
            })
            .await
            .map_err(|e| create_error(&e))?
            .ready()
            .map_err(|p| format!("{} is still starting", p.id))?;
        Ok(self.row(info).await)
    }

    /// Cancels a create still running, by the `create_id` it was started
    /// with (or the Space's id or name). Returns once what it made is
    /// removed; idempotent (`not_creating` when nothing is in flight).
    pub async fn cancel_create(&self, key: &str) -> CmdResult<cua_spaces::CancelOutcome> {
        self.spaces().cancel_create(key).await.map_err(msg)
    }

    /// The New Space wizard's GPU choices on this machine: the first GPU
    /// option of each local runtime that has one.
    pub async fn gpu_support(&self) -> Vec<cua_spaces_app_core::wizard::GpuChoice> {
        gpu_choices(&self.spaces().gpu_support(&On::Local).await)
    }

    /// Your machines that provide Spaces (`WizardEnv.hosts`, the New Space
    /// "Run on" menu); none when they cannot be listed.
    pub async fn list_hosts(&self) -> Vec<cua_spaces_app_core::wizard::SpaceHost> {
        self.spaces()
            .hosts()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(space_host)
            .collect()
    }

    /// Deletes a Space the app created (cloud: the sandbox is deleted and
    /// metering stops; local: the instance is deleted). A Space added by
    /// address is only forgotten.
    pub async fn delete_space(&self, space: &str) -> CmdResult<String> {
        let spaces = self.spaces();
        let id = spaces.resolve(space).map_err(msg)?.to_string();
        self.forget_hotspot(&id);
        spaces.delete(&id).await.map_err(msg)
    }

    /// Turns a Space off (`on` false: suspended or stopped, as its provider
    /// can) or on again. A hotspot on it ends when it goes off.
    pub async fn set_space_power(
        &self,
        space: &str,
        on: bool,
    ) -> CmdResult<cua_spaces::SpacePower> {
        let spaces = self.spaces();
        let id = spaces.resolve(space).map_err(msg)?.to_string();
        if on {
            spaces.start(&id).await.map_err(msg)
        } else {
            self.forget_hotspot(&id);
            spaces.stop(&id).await.map_err(msg)
        }
    }

    /// Unregisters without touching the sandbox.
    pub async fn remove_space(&self, space: &str) -> CmdResult<()> {
        let spaces = self.spaces();
        let id = spaces.resolve(space).map_err(msg)?.to_string();
        self.forget_hotspot(&id);
        spaces.remove(&id).await.map(drop).map_err(msg)
    }

    /// Extends a Fleet Space's lease.
    pub async fn keep_alive_space(&self, space: &str, seconds: u64) -> CmdResult<()> {
        let spaces = self.spaces();
        match spaces.resolve(space).map_err(msg)? {
            cua_spaces::SpaceId::Cloud { name, namespace } => {
                let fleet = self
                    .fleet_client
                    .as_ref()
                    .ok_or("Fleet is not configured")?;
                // `cloud:<name>` carries no namespace: find the claim.
                let namespace = match namespace {
                    Some(ns) => ns,
                    None => fleet
                        .find_claims(&name)
                        .await
                        .map_err(msg)?
                        .into_iter()
                        .next()
                        .map(|c| c.metadata.namespace)
                        .ok_or_else(|| format!("cloud:{name} not found"))?,
                };
                fleet
                    .keep_alive(&namespace, &name, Duration::from_secs(seconds))
                    .await
                    .map(drop)
                    .map_err(msg)
            }
            other => Err(format!("{other} has no lease to extend")),
        }
    }

    async fn space(&self, space: &str) -> CmdResult<Space> {
        self.spaces().space(space).await.map_err(msg)
    }

    // ------------------------------------------------------- space ops

    /// A PNG/JPEG `data:` URL of the primary display.
    pub async fn space_screenshot(
        &self,
        space: &str,
        max_dimension: Option<u32>,
    ) -> CmdResult<String> {
        let s = self.space(space).await?;
        let shot = s
            .spacesd()
            .map_err(msg)?
            .screenshot(cua_spacesd_client::ScreenshotOptions {
                max_dimension: max_dimension.unwrap_or(1280),
                ..Default::default()
            })
            .await
            .map_err(msg)?;
        Ok(data_url(shot.format, &shot.image))
    }

    /// Sends host files into `~/Downloads[/subdir]`, sha256-verified by the
    /// Space.
    pub async fn send_files(
        &self,
        space: &str,
        paths: &[String],
        subdir: Option<String>,
    ) -> CmdResult<Vec<SentFile>> {
        if paths.is_empty() {
            return Err("nothing to send".into());
        }
        let s = self.space(space).await?;
        let mut out = Vec::new();
        for p in paths {
            let report = s
                .send_file(
                    Path::new(p),
                    SendFileOptions {
                        subdir: subdir.clone().unwrap_or_default(),
                        ..Default::default()
                    },
                )
                .await
                .map_err(msg)?;
            for f in report.files {
                out.push(SentFile {
                    name: Path::new(&f.path)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    dest: f.path,
                    bytes: f.size,
                    sha256: f.sha256,
                });
            }
        }
        Ok(out)
    }

    /// The Space's windows.
    pub async fn list_remote_windows(&self, space: &str) -> CmdResult<Vec<RemoteWindow>> {
        let s = self.space(space).await?;
        let windows = s.windows(None).await.map_err(msg)?;
        Ok(windows
            .into_iter()
            .map(|w| RemoteWindow {
                width_px: (w.bounds[2] > 0.0).then_some(w.bounds[2].round() as u32),
                height_px: (w.bounds[3] > 0.0).then_some(w.bounds[3].round() as u32),
                id: w.window_id,
                app_name: w.app_name.clone(),
                title: w.title,
                visible: w.on_screen,
                app_id: if w.app_id.is_empty() {
                    w.app_name.to_ascii_lowercase().replace(' ', "-")
                } else {
                    w.app_id
                },
                target_epoch: w.epoch,
                pid: (w.pid > 0).then_some(w.pid),
            })
            .collect())
    }

    /// Memory and storage use now (the app core's `SpaceUsage` shape).
    pub async fn space_usage(&self, space: &str) -> CmdResult<serde_json::Value> {
        let s = self.space(space).await?;
        let u = s.usage().await.map_err(msg)?;
        Ok(serde_json::json!({
            "memoryUsed": u.memory_used,
            "memoryTotal": u.memory_total,
            "memoryLimited": u.memory_limited,
            "diskUsed": u.disk_used,
            "diskTotal": u.disk_total,
            "diskLimited": u.disk_limited,
        }))
    }

    /// The Space's primary display size, from its display list.
    pub async fn space_primary_display(&self, space: &str) -> CmdResult<Option<DisplaySize>> {
        let s = self.space(space).await?;
        let displays = s.displays().await.map_err(msg)?;
        Ok(displays
            .first()
            .filter(|d| d.width_px > 0 && d.height_px > 0)
            .map(|d| DisplaySize {
                width_px: d.width_px,
                height_px: d.height_px,
            }))
    }

    /// The icon the Space's desktop shows for an app, as a `data:` URL, or
    /// `None` (see [`Core::space_app_icons`]).
    pub async fn space_app_icon(
        &self,
        space: &str,
        app_name: &str,
        app_id: &str,
        pid: u32,
    ) -> CmdResult<Option<String>> {
        let request = AppIconRequest {
            app_name: app_name.into(),
            app_id: app_id.into(),
            pid,
        };
        Ok(self
            .space_app_icons(space, vec![request])
            .await?
            .pop()
            .flatten())
    }

    /// Icons for many windows' apps (the SDK's `Space::app_icons`, through
    /// its one icon cache) as `data:` URLs, in request order.
    pub async fn space_app_icons(
        &self,
        space: &str,
        requests: Vec<AppIconRequest>,
    ) -> CmdResult<Vec<Option<String>>> {
        let s = self.space(space).await?;
        let requests: Vec<cua_spaces::IconRequest> = requests
            .into_iter()
            .map(|r| cua_spaces::IconRequest {
                app_name: r.app_name,
                app_id: r.app_id,
                pid: r.pid,
            })
            .collect();
        let icons = s.app_icons(&requests).await.map_err(msg)?;
        Ok(icons
            .into_iter()
            .map(|i| {
                i.map(|i| {
                    format!(
                        "data:{};base64,{}",
                        i.content_type,
                        base64::engine::general_purpose::STANDARD.encode(&i.bytes)
                    )
                })
            })
            .collect())
    }

    /// A small capture of one remote window (the SDK's
    /// `Space::window_thumbnail`, reused for a few seconds), or `None`.
    pub async fn remote_window_thumbnail(
        &self,
        space: &str,
        window_id: &str,
        epoch: u64,
    ) -> CmdResult<Option<String>> {
        let s = self.space(space).await?;
        let jpeg = s
            .window_thumbnail(window_id, epoch, 480)
            .await
            .ok()
            .flatten();
        Ok(jpeg.map(|b| data_url(pb::ImageFormat::Jpeg, &b)))
    }

    /// Agent runs in a Space.
    pub async fn list_space_agents(
        &self,
        space: &str,
    ) -> CmdResult<Vec<crate::agents::SpaceAgentRun>> {
        let s = self.space(space).await?;
        let runs = s.agents().await.map_err(msg)?.list().await.map_err(msg)?;
        Ok(runs.iter().map(crate::agents::space_agent_run).collect())
    }

    /// Installs `harness` in a Linux Space (pinned tools in `~/.cua/tools`,
    /// bins in `~/.cua/bin`, plus the cua skills), then opens its
    /// interactive CLI in a terminal on the Space's desktop. The user signs
    /// in inside that terminal: no host credential is read or copied.
    pub async fn launch_agent_terminal(&self, space: &str, harness: &str) -> CmdResult<()> {
        let h = cua_spaces::agents::harness::harness(harness)
            .ok_or_else(|| format!("unknown coding agent {harness:?}"))?;
        let cli = h.cli.ok_or_else(|| {
            format!(
                "{} has no interactive terminal; start it with `cua agent run` instead",
                h.name
            )
        })?;
        let s = self.space(space).await?;
        if matches!(s.os_family(), pb::OsFamily::Macos | pb::OsFamily::Windows) {
            return Err(format!(
                "{} opens in a terminal only in Linux Spaces for now",
                h.name
            ));
        }
        let agents = s.agents().await.map_err(msg)?;
        agents
            .ensure(h.installs, |p| {
                tracing::debug!(id = %p.id, phase = %p.phase, "installing {}: {}", h.id, p.detail)
            })
            .await
            .map_err(|e| format!("could not install {}: {e}", h.name))?;
        if let Err(e) = agents.install_skills(h).await {
            tracing::warn!("cua skills for {}: {e}", h.id);
        }
        let guest = s.spacesd().map_err(msg)?;
        let has_xterm = guest
            .run(
                cua_spacesd_client::Command::shell("command -v xterm >/dev/null 2>&1")
                    .timeout(Duration::from_secs(30)),
            )
            .await
            .map_err(msg)?
            .status
            .success();
        if !has_xterm {
            return Err(format!(
                "{} is installed, but this Space has no xterm to open it in",
                h.name
            ));
        }
        let handle = guest
            .spawn(
                cua_spacesd_client::Command::shell(agent_terminal_script(h.name, cli))
                    .tag(format!("cua-agent-terminal/{}", h.id)),
            )
            .await
            .map_err(msg)?;
        handle.detach();
        Ok(())
    }

    // --------------------------------------------------------- streams

    /// Mints a media ticket the webview attaches to (wire v2).
    pub async fn open_stream(
        &self,
        space: &str,
        target: StreamTargetArg,
        opts: StreamOpts,
    ) -> CmdResult<StreamTicketInfo> {
        let s = self.space(space).await?;
        let req = open_media_request(&s, &target, &opts)?;
        let audio = req.audio.is_some();
        if s.provider() == Provider::Cloud {
            // The Fleet gateway needs a bearer + claim header on the media
            // socket, which a webview cannot send: bridge through the daemon.
            let daemon = self.daemon(true).await?;
            let json = serde_json::to_string(&req).map_err(msg)?;
            let r = daemon
                .open_media_bridge(&s.id().to_string(), Some(json))
                .await
                .map_err(|e| format!("cua daemon media bridge: {e}"))?;
            let raw: pb::OpenMediaResponse =
                serde_json::from_str(&r.open_media_response_json).map_err(msg)?;
            return Ok(ticket_info(&s, raw, r.ws_url, r.ticket, "daemon", audio));
        }
        let raw = s
            .spacesd()
            .map_err(msg)?
            .stream()
            .open_media(req)
            .await
            .map_err(|st| msg(cua_spacesd_client::Error::from(st)))?
            .into_inner();
        let ws_path = if raw.ws_path.is_empty() {
            format!("/media?ticket={}", raw.ticket)
        } else {
            raw.ws_path.clone()
        };
        let ws_url = s.websocket_url(&ws_path).map_err(msg)?;
        let ticket = raw.ticket.clone();
        Ok(ticket_info(&s, raw, ws_url, ticket, "direct", audio))
    }

    /// `CloseMedia`.
    pub async fn close_stream(&self, space: &str, media_session_id: &str) -> CmdResult<()> {
        self.space(space)
            .await?
            .close_stream(media_session_id)
            .await
            .map_err(msg)
    }

    // -------------------------------------------------------- teleport

    /// What `app` would send (read-only, host side).
    pub async fn teleport_manifest(&self, app: &str, scope: &str) -> CmdResult<TransferManifest> {
        let scope = TeleportScope::parse(Some(scope)).map_err(msg)?;
        let sessions = self.app_sessions.clone();
        let app = app.to_string();
        let m = tokio::task::spawn_blocking(move || sessions.manifest(&app, scope))
            .await
            .map_err(msg)?
            .map_err(msg)?;
        Ok(TransferManifest::from_sdk(&m))
    }

    /// Exports the consented items and imports them into the Space. The
    /// consent ladder's outcome becomes a `cua_spaces` `Approval`, which
    /// refuses items not in the manifest and sensitive items without
    /// `acknowledge_sensitive`.
    pub async fn teleport_push(
        &self,
        app: &str,
        scope: &str,
        space: &str,
        include: &[String],
        acknowledge_sensitive: bool,
    ) -> CmdResult<TeleportResult> {
        let scope = TeleportScope::parse(Some(scope)).map_err(msg)?;
        let s = self.space(space).await?;
        let sessions = self.app_sessions.clone();
        let app_c = app.to_string();
        let manifest = tokio::task::spawn_blocking(move || sessions.manifest(&app_c, scope))
            .await
            .map_err(msg)?
            .map_err(msg)?;
        let space_id = s.id().to_string();
        let approval = if include.is_empty() {
            manifest.approving_default(&space_id, acknowledge_sensitive)
        } else {
            manifest.approving(&space_id, include, acknowledge_sensitive)
        }
        .map_err(msg)?;
        let receipt = s
            .teleport(
                self.app_sessions.clone(),
                &approval,
                ImportOptions {
                    replace_existing: false,
                    close_running_app: true,
                    launch_after: true,
                    // This simpler drag/drop teleport has no review sheet to
                    // offer "Save to Keyvault" from; `teleport_app_run`'s
                    // picker flow is the one that does.
                    save_to_keyvault: false,
                    // This push path has no relay-plaintext consent screen:
                    // an unsealed relay delivery is refused (S1).
                    relay_plaintext_ack: false,
                },
            )
            .await
            .map_err(msg)?;
        Ok(TeleportResult {
            ok: true,
            provider_id: receipt.app,
            launched: receipt.launched,
            pid: None,
            imported: receipt.imported,
            skipped: receipt.skipped,
        })
    }

    /// Teleport providers on this host.
    pub fn teleport_providers(&self) -> Vec<cua_spaces_ext::teleport::ProviderInfo> {
        self.app_sessions.catalog()
    }

    // ------------------------------------------- "Teleport an app…"

    /// The catalog of apps on this machine (the SDK's `cua_teleport::ux`),
    /// narrowed to `space`'s OS and CPU when given.
    pub async fn teleport_app_catalog(
        &self,
        space: Option<&str>,
    ) -> CmdResult<Vec<cua_teleport::ux::CatalogEntry>> {
        let hint = match space {
            // OS and CPU from the Space's capabilities: no guest round trip.
            Some(id) => self
                .space(id)
                .await?
                .app_teleport_hint()
                .await
                .map_err(msg)?,
            None => Default::default(),
        };
        let req = cua_teleport::ux::CatalogRequest {
            probe_providers: self.teleport_app_roots.is_none(),
            roots: self.teleport_app_roots.clone(),
            hint,
            recents_path: self.teleport_recents.clone(),
        };
        let sessions = self.app_sessions.clone();
        tokio::task::spawn_blocking(move || cua_teleport::ux::catalog(sessions.registry(), &req))
            .await
            .map_err(msg)?
            .map_err(msg)
    }

    /// Warms the picker's app list and icons in the background (the SDK's
    /// caches), so the first "Teleport an app…" opens on cached ones.
    pub fn teleport_prefetch(&self) {
        let roots = self.teleport_app_roots.clone();
        let _ = std::thread::Builder::new()
            .name("cua-teleport-prefetch".into())
            .spawn(move || cua_teleport::ux::prefetch(roots));
    }

    /// The catalog row for a dropped app (bundle, `.desktop`, shortcut).
    pub fn teleport_entry_for_path(&self, path: &str) -> CmdResult<cua_teleport::ux::CatalogEntry> {
        cua_teleport::ux::entry_for_path(self.app_sessions.registry(), path, &Default::default())
            .map_err(msg)
    }

    /// The catalog row for a dragged window: its bundle when known, else its
    /// owner's name.
    pub fn teleport_entry_for_window(
        &self,
        bundle_path: Option<&str>,
        app_name: &str,
    ) -> cua_teleport::ux::CatalogEntry {
        let registry = self.app_sessions.registry();
        bundle_path
            .and_then(|p| cua_teleport::ux::entry_for_path(registry, p, &Default::default()).ok())
            .unwrap_or_else(|| {
                cua_teleport::ux::entry_for_name(registry, app_name, &Default::default())
            })
    }

    /// What teleporting `entry` into `space` will do (steps and consent).
    pub async fn teleport_app_plan(
        &self,
        space: &str,
        entry: &cua_teleport::ux::CatalogEntry,
        options: &cua_teleport::ux::PlanOptions,
    ) -> CmdResult<cua_teleport::ux::TeleportPlan> {
        let s = self.space(space).await?;
        s.plan_app_teleport(self.app_sessions.clone(), entry, options)
            .await
            .map_err(msg)
    }

    /// Runs an approved plan, reporting each [`cua_teleport::ux::RunEvent`],
    /// then records the app in the recents.
    pub async fn teleport_app_run(
        &self,
        space: &str,
        plan: cua_teleport::ux::TeleportPlan,
        consent: cua_teleport::ux::Consent,
        progress: impl FnMut(cua_teleport::ux::RunEvent) + Send,
    ) -> CmdResult<cua_teleport::ux::RunReport> {
        let approved = plan.approve(consent).map_err(msg)?;
        let s = self.space(space).await?;
        let report = s
            .run_app_teleport(self.app_sessions.clone(), &approved, progress)
            .await
            .map_err(msg)?;
        if let Err(e) =
            cua_teleport::ux::record_recent(self.teleport_recents.clone(), &report.app_id)
        {
            tracing::warn!("could not record the teleport recent: {e}");
        }
        Ok(report)
    }

    // --------------------------------------------------------- hotspot

    fn forget_hotspot(&self, id: &str) {
        let mut h = self.hotspot.lock().unwrap_or_else(|p| p.into_inner());
        if h.as_deref() == Some(id) {
            *h = None;
        }
    }

    /// Shares this machine's network with one Space (replaces any other).
    pub async fn start_hotspot(&self, space: &str) -> CmdResult<HotspotStatus> {
        let spaces = self.spaces();
        let id = spaces.resolve(space).map_err(msg)?.to_string();
        let _ = spaces.hotspot_stop(None).await;
        spaces
            .hotspot_start(&id, HotspotOptions::default())
            .await
            .map_err(msg)?;
        *self.hotspot.lock().unwrap_or_else(|p| p.into_inner()) = Some(id);
        Ok(self.hotspot_status())
    }

    /// Stops sharing. Idempotent.
    pub async fn stop_hotspot(&self) -> CmdResult<HotspotStatus> {
        self.spaces().hotspot_stop(None).await.map_err(msg)?;
        *self.hotspot.lock().unwrap_or_else(|p| p.into_inner()) = None;
        Ok(self.hotspot_status())
    }

    /// Current hotspot.
    pub fn hotspot_status(&self) -> HotspotStatus {
        let id = self
            .hotspot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        HotspotStatus {
            active: id.is_some(),
            space_id: id,
        }
    }
}

fn data_url(format: pb::ImageFormat, bytes: &[u8]) -> String {
    let mime = match format {
        pb::ImageFormat::Jpeg => "image/jpeg",
        pb::ImageFormat::Webp => "image/webp",
        _ => "image/png",
    };
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

fn codec_of(name: &str) -> CmdResult<pb::MediaCodec> {
    match name {
        "h264" => Ok(pb::MediaCodec::H264),
        "bgra" => Ok(pb::MediaCodec::Bgra),
        "png" => Ok(pb::MediaCodec::Png),
        other => Err(format!("unknown codec {other:?}")),
    }
}

fn codec_name(codec: i32) -> String {
    match pb::MediaCodec::try_from(codec).unwrap_or_default() {
        pb::MediaCodec::H264 => "h264",
        pb::MediaCodec::Bgra => "bgra",
        pb::MediaCodec::Png => "png",
        pb::MediaCodec::Unspecified => "unspecified",
    }
    .into()
}

/// The one `OpenMediaRequest` both paths (direct and daemon bridge) send.
pub fn open_media_request(
    space: &Space,
    target: &StreamTargetArg,
    o: &StreamOpts,
) -> CmdResult<pb::OpenMediaRequest> {
    let target = match target {
        StreamTargetArg::Display { display_id } => {
            space.require("desktop_stream").map_err(msg)?;
            pb::media_target::Target::DisplayId(
                display_id.clone().unwrap_or_else(|| "primary".into()),
            )
        }
        StreamTargetArg::Window { window_id } => {
            space.require("window_stream").map_err(msg)?;
            pb::media_target::Target::Window(pb::WindowRef {
                id: window_id.clone(),
                epoch: 0,
            })
        }
    };
    let audio = o.audio.unwrap_or(false);
    if audio {
        space.require("audio.desktop").map_err(msg)?;
    }
    let policy = match o.policy.as_deref() {
        None | Some("allow_activation") => pb::SessionPolicy::AllowActivation,
        Some("background_only") => pb::SessionPolicy::BackgroundOnly,
        Some("view_only") => pb::SessionPolicy::ViewOnly,
        Some(other) => return Err(format!("unknown stream policy {other:?}")),
    };
    Ok(pb::OpenMediaRequest {
        target: Some(pb::MediaTarget {
            target: Some(target),
        }),
        codecs: o
            .codecs
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(|c| codec_of(c).map(|c| c as i32))
            .collect::<CmdResult<_>>()?,
        max_fps: o.max_fps.unwrap_or(0),
        max_dimension: o.max_dimension.unwrap_or(0),
        bitrate_kbps: 0,
        policy: policy as i32,
        geometry_control: if o.geometry_control.unwrap_or(false) {
            pb::GeometryControl::Bidirectional as i32
        } else {
            pb::GeometryControl::ObserveOnly as i32
        },
        ticket_ttl: Some(pbjson_duration(Duration::from_secs(120))),
        prefer_quic: false,
        audio: audio.then(|| pb::AudioOptions {
            enabled: true,
            ..Default::default()
        }),
        disable_video: false,
        // Attribute this viewer's input to its presence identity.
        presence_participant_id: space.presence_participant().unwrap_or_default(),
    })
}

fn pbjson_duration(d: Duration) -> cua_proto::wkt::Duration {
    cua_proto::wkt::Duration {
        seconds: d.as_secs() as i64,
        nanos: 0,
    }
}

fn ticket_info(
    s: &Space,
    raw: pb::OpenMediaResponse,
    ws_url: String,
    ticket: String,
    via: &'static str,
    audio: bool,
) -> StreamTicketInfo {
    let size = raw
        .geometry
        .as_ref()
        .and_then(|g| g.frame_size.as_ref())
        .map(|s| [s.width, s.height])
        .unwrap_or([0, 0]);
    StreamTicketInfo {
        space_id: s.id().to_string(),
        media_session_id: raw.media_session_id.clone(),
        ws_url,
        ticket,
        ticket_expires_at: raw.ticket_expires_at.as_ref().map(|t| {
            let at = std::time::UNIX_EPOCH + Duration::from_secs(t.seconds.max(0) as u64);
            format_rfc3339(at)
        }),
        codec: codec_name(raw.codec),
        wire_version: raw.wire_version,
        frame_size: size,
        via,
        audio,
    }
}

fn format_rfc3339(at: std::time::SystemTime) -> String {
    // Seconds precision, UTC, without a date crate.
    let secs = at
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-to-civil.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    #[test]
    fn agent_terminal_script_quotes_and_puts_cua_bin_first() {
        let s = super::agent_terminal_script("Bob's Agent", "goose");
        assert_eq!(
            s,
            "export DISPLAY=\"${DISPLAY:-:1}\"; exec xterm -T 'Bob'\\''s Agent' -e sh -c \
             'PATH=\"$HOME/.cua/bin:$PATH\"; export PATH; exec goose'"
        );
    }

    use super::*;

    #[test]
    fn rfc3339_matches_known_instants() {
        let at = std::time::UNIX_EPOCH + Duration::from_secs(1_758_300_000);
        assert_eq!(format_rfc3339(at), "2025-09-19T16:40:00Z");
        assert_eq!(
            format_rfc3339(std::time::UNIX_EPOCH),
            "1970-01-01T00:00:00Z"
        );
        let leap = std::time::UNIX_EPOCH + Duration::from_secs(951_782_400); // 2000-02-29
        assert_eq!(format_rfc3339(leap), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn stream_targets_deserialize_from_the_webview_shape() {
        let d: StreamTargetArg = serde_json::from_str(r#"{"kind":"display"}"#).unwrap();
        assert_eq!(d, StreamTargetArg::Display { display_id: None });
        let w: StreamTargetArg =
            serde_json::from_str(r#"{"kind":"window","windowId":"0x1f"}"#).unwrap();
        assert_eq!(
            w,
            StreamTargetArg::Window {
                window_id: "0x1f".into()
            }
        );
    }

    #[test]
    fn codecs_map_and_refuse_unknown_names() {
        assert_eq!(codec_of("h264").unwrap(), pb::MediaCodec::H264);
        assert!(codec_of("vp9").is_err());
        assert_eq!(codec_name(pb::MediaCodec::Png as i32), "png");
    }
}

#[cfg(test)]
mod display_name_tests {
    use super::display_name;

    #[test]
    fn a_container_hostname_gives_way_to_the_given_name() {
        assert_eq!(
            display_name("local:cua-e2e-desk", "69af41d9344c"),
            "cua-e2e-desk"
        );
        assert_eq!(display_name("local:desk", "My desk"), "My desk");
        assert_eq!(
            display_name("direct:10.0.0.5:3211", "69af41d9344c"),
            "69af41d9344c"
        );
    }
}
