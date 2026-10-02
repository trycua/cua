//! `Spaces` and `Space`: the Spaces registry and primitives of `cua-spaces`
//! for every language, in both topologies.
//!
//! | | embedded (`Cua.embedded`) | daemon (`Cua.connect`) |
//! |---|---|---|
//! | registry / lifecycle | `cua_spaces::Spaces` in this process | `SpaceService` RPCs |
//! | typed primitives (exec, files, services, streams, presence) | `cua_spaces::Space` over the Space's spacesd | the same `cua_spaces::Space` code over the daemon's per-Space env passthrough (`SpaceService.ConnectSpace`); the Space's credentials never leave the daemon |
//! | host-effect primitives (teleport, hotspot, agents) | the Spaces MCP tool, in process | the same tool in the daemon (`SpaceService.CallSpaceTool`), so hotspots outlive the caller and host reads happen in one place |
//!
//! Tool-backed results are parsed from the tool's JSON, and direct results
//! are serialized through the same serde shape, so both modes return
//! identical records. [`SPACES_TOOL_METHODS`] maps every contract tool to
//! the SDK method that covers it (checked by a test).

use super::teleport_types::TeleportManifest;
use super::{AudioSink, Backend, FrameSink, MediaEvent, run};
use crate::{CuaError, Result};
use cua_daemon::client::DaemonClient;
use cua_proto::daemon::v1 as dpb;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Every Spaces contract tool and the SDK method(s) covering it
/// (`Class.method`). `Spaces.call_tool_json` reaches any of them verbatim.
pub const SPACES_TOOL_METHODS: &[(&str, &str)] = &[
    ("add_space", "Spaces.add"),
    ("remove_space", "Spaces.remove"),
    ("list_spaces", "Spaces.list"),
    ("create_space", "Spaces.create"),
    ("delete_space", "Spaces.delete"),
    ("stop_space", "Spaces.stop"),
    ("start_space", "Spaces.start"),
    ("space_bash", "Space.bash"),
    ("space_write", "Space.write"),
    ("upload", "Space.upload"),
    ("send_file", "Space.send_file"),
    ("download", "Space.download"),
    ("stream_endpoint", "Space.open_stream"),
    ("list_space_windows", "Space.windows"),
    ("stream_space_window", "Spaces.call_tool_json"),
    ("show_space_pip", "Spaces.call_tool_json"),
    ("hide_space_pip", "Spaces.call_tool_json"),
    ("open_space_viewer", "Spaces.call_tool_json"),
    ("list_tools", "Space.list_tools"),
    ("call_tool", "Space.call_tool"),
    ("agent_start", "Space.agent_start"),
    ("agent_message", "Space.agent_message"),
    ("agent_status", "Space.agent_status"),
    ("agent_events", "Space.agent_events"),
    ("agent_interrupt", "Space.agent_interrupt"),
    ("agent_stop", "Space.agent_stop"),
    ("agent_list", "Space.agent_list"),
    ("agent_capabilities", "Spaces.agent_capabilities"),
    ("persistent_agent_create", "Spaces.persistent_agent_create"),
    ("persistent_agent_list", "Spaces.persistent_agents"),
    ("persistent_agent_remove", "Spaces.persistent_agent_remove"),
    ("persistent_agent_send", "Spaces.persistent_agent_send"),
    ("persistent_agent_save", "Spaces.persistent_agent_save"),
    ("agent_pause", "Spaces.agent_pause"),
    ("agent_resume", "Spaces.agent_resume"),
    ("routine_add", "Spaces.routine_add"),
    ("routine_list", "Spaces.routines"),
    ("routine_remove", "Spaces.routine_remove"),
    ("routine_set_enabled", "Spaces.routine_set_enabled"),
    ("notify_user", "Spaces.notify_user"),
    ("notifications_list", "Spaces.notifications"),
    ("notifications_ack", "Spaces.notifications_ack"),
    ("computer_access_grant", "Spaces.computer_access_grant"),
    ("computer_access_revoke", "Spaces.computer_access_revoke"),
    ("computer_access_list", "Spaces.computer_access"),
    ("teleport_manifest", "Space.teleport_manifest"),
    ("teleport_app", "Space.teleport"),
    ("request_site_login", "Space.request_site_login"),
    ("hotspot_start", "Space.start_hotspot"),
    ("hotspot_stop", "Space.stop_hotspot"),
    ("hotspot_status", "Space.hotspot_status"),
    ("volume_ls", "Spaces.volume_ls"),
    ("volume_read", "Spaces.volume_read"),
    ("volume_write", "Spaces.volume_write"),
    ("volume_delete", "Spaces.volume_delete"),
    ("volume_history", "Spaces.volume_history"),
    ("volume_restore", "Spaces.volume_restore"),
    ("volume_grant", "Spaces.volume_grant"),
    ("volume_revoke", "Spaces.volume_revoke"),
    ("volume_grants", "Spaces.volume_grants"),
    ("volume_request_access", "Spaces.volume_request_access"),
    ("volume_requests", "Spaces.volume_requests"),
    ("volume_approve", "Spaces.volume_approve"),
    ("volume_deny", "Spaces.volume_deny"),
    ("volume_audit", "Spaces.volume_audit"),
    ("volume_storage", "Spaces.volume_storage"),
    ("volume_storage_set", "Spaces.volume_storage_set"),
    ("volume_mount_status", "Spaces.volume_mount_status"),
    ("volume_mount", "Spaces.volume_mount"),
    ("volume_unmount", "Spaces.volume_unmount"),
    ("volume_sync_status", "Spaces.volume_sync_status"),
    ("volume_sync_events", "Spaces.volume_sync_events"),
    ("volume_sync_resolve", "Spaces.volume_sync_resolve"),
    ("volume_cache_stats", "Spaces.volume_cache_stats"),
    ("volume_cache_set", "Spaces.volume_cache_set"),
    ("volume_cache_clear", "Spaces.volume_cache_clear"),
    ("share_space", "Space.share"),
    ("unshare_space", "Space.unshare"),
    ("space_shares", "Space.shares"),
    ("relay_register_space", "Spaces.relay_register"),
    ("relay_unregister_space", "Spaces.relay_unregister"),
    ("cloud_status", "Spaces.cloud_status"),
    ("cloud_connect", "Spaces.cloud_connect"),
    ("cloud_test", "Spaces.cloud_test"),
    ("cloud_disconnect", "Spaces.cloud_disconnect"),
    ("cloud_sweep", "Spaces.cloud_sweep"),
];

/// SDK methods that have no contract tool (SDK-only surface).
pub const SPACES_SDK_ONLY_METHODS: &[&str] = &[
    "Spaces.resolve",
    "Spaces.space",
    "Spaces.list_tools_json",
    "Spaces.call_tool_json",
    "Space.home",
    "Space.screenshot",
    "Space.close_stream",
    "Space.attach_stream",
    "Space.stream_session",
    "Space.join_presence",
];

/// One row of [`SPACES_TOOL_METHODS`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpacesToolMethod {
    /// Contract tool name.
    pub tool: String,
    /// `Class.method` (Rust / Python spelling; camelCase in TS, Swift and
    /// Kotlin).
    pub method: String,
}

/// Which SDK method covers each Spaces contract tool. Language test suites
/// check every listed method exists on the generated class.
#[uniffi::export]
pub fn spaces_tool_methods() -> Vec<SpacesToolMethod> {
    SPACES_TOOL_METHODS
        .iter()
        .map(|(tool, method)| SpacesToolMethod {
            tool: (*tool).into(),
            method: (*method).into(),
        })
        .collect()
}

// ------------------------------------------------------------------ records

/// A registered Space.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct SpaceInfo {
    /// Id, for example `direct:10.0.0.5:3211` or `cloud:<name>` (the
    /// sandbox ref scheme).
    pub id: String,
    /// Display name.
    pub name: String,
    /// `cloud`, `local`, `direct` or `relay` (the location words).
    pub provider: String,
    /// spacesd version at the last handshake.
    pub spacesd_version: String,
    /// Supported features at the last handshake.
    pub features: Vec<String>,
    /// Guest OS family at the last handshake: `linux`, `macos`, `windows`,
    /// or empty when not reported.
    #[serde(default)]
    pub os: String,
    /// Guest OS product or distribution at the last handshake ("Ubuntu",
    /// "macOS"), or empty when not reported.
    #[serde(default)]
    pub os_name: String,
    /// The guest's full OS string at the last handshake ("Ubuntu 24.04.3
    /// LTS", "macOS 26.5.2 (25F84)"), or empty from older drivers.
    #[serde(default)]
    pub os_pretty_name: String,
    /// The image the sandbox runs ("ghcr.io/trycua/linux:24.04"), or empty
    /// when unknown (a Space added by address).
    #[serde(default)]
    pub image: String,
    /// The digest of the variant that runs ("sha256:..."), or empty.
    #[serde(default)]
    pub image_digest: String,
    /// `container` or `vm`, or empty when unknown.
    #[serde(default)]
    pub kind: String,
    /// The guest's CPU architecture (`arm64`, `amd64`), or empty when
    /// unknown.
    #[serde(default)]
    pub arch: String,
    /// Declared services (never `env`), reachable with `list_tools` /
    /// `call_tool` (`service`). A Space without cua-spacesd has an empty
    /// `features` list and only these.
    #[serde(default)]
    pub services: Vec<String>,
    /// When it was added (RFC 3339).
    #[serde(default)]
    pub added_at: Option<String>,
    /// For a Space one of your machines provides (created with
    /// `on="host:<machine>"`): that host's relay machine id; empty
    /// otherwise. Lists group these Spaces under their host.
    #[serde(default)]
    pub host: String,
    /// The host's display name, when known.
    #[serde(default)]
    pub host_name: String,
    /// How it turns off and on again ([`Spaces::stop`], [`Spaces::start`]):
    /// `suspend` (its memory is kept), `stop` (its disk is kept), or empty
    /// when it cannot (a cloud Space, a Space added by address).
    #[serde(default)]
    pub power: String,
    /// `running`, `suspended` or `stopped` as cua last recorded it; empty
    /// when unknown (a Space one of your machines provides answers while it
    /// runs).
    #[serde(default)]
    pub power_state: String,
    /// A Space in your own cloud: the provider (`aws`, `gcp`, `modal`);
    /// empty otherwise.
    #[serde(default)]
    pub cloud: String,
    /// Where it runs ("AWS · us-west-2").
    #[serde(default)]
    pub cloud_place: String,
    /// How `delete` would delete it permanently from here: `here` (this
    /// device created it), `host:<machine>` (the device that created it,
    /// through the relay) or `elsewhere` (only there; `remove` keeps it).
    /// Empty for other Spaces.
    #[serde(default)]
    pub cloud_delete: String,
}

impl From<dpb::Space> for SpaceInfo {
    fn from(s: dpb::Space) -> Self {
        let provider = cua_spaces::SpaceId::parse(&s.id)
            .map(|id| id.provider().as_str().to_string())
            .unwrap_or_default();
        SpaceInfo {
            added_at: s.added_at.as_ref().map(|t| {
                let at = std::time::UNIX_EPOCH
                    + Duration::new(t.seconds.max(0) as u64, t.nanos.max(0) as u32);
                humantime_rfc3339(at)
            }),
            id: s.id,
            name: s.name,
            provider,
            spacesd_version: s.spacesd_version,
            features: s.features,
            os: s.os,
            os_name: s.os_name,
            os_pretty_name: s.os_pretty_name,
            image: s.image,
            image_digest: s.image_digest,
            kind: s.kind,
            arch: s.arch,
            services: s.services,
            host: s.host,
            host_name: s.host_name,
            power: s.power,
            power_state: s.power_state,
            cloud: s.cloud,
            cloud_place: s.cloud_place,
            cloud_delete: s.cloud_delete,
        }
    }
}

impl From<cua_spaces::SpaceInfo> for SpaceInfo {
    fn from(s: cua_spaces::SpaceInfo) -> Self {
        SpaceInfo {
            id: s.id,
            name: s.name,
            provider: s.provider.as_str().into(),
            spacesd_version: s.spacesd_version,
            features: s.features,
            os: s.os,
            os_name: s.os_name,
            os_pretty_name: s.os_pretty_name,
            image: s.image,
            image_digest: s.image_digest,
            kind: s.kind,
            arch: s.arch,
            services: s.services,
            added_at: s.added_at,
            host: s.host,
            host_name: s.host_name,
            power: s.power,
            power_state: s.power_state,
            cloud: s.cloud,
            cloud_place: s.cloud_place,
            cloud_delete: s.cloud_delete,
        }
    }
}

fn humantime_rfc3339(at: std::time::SystemTime) -> String {
    let secs = at
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Civil-from-days (Howard Hinnant), UTC.
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Options for [`Spaces::create`]: the same location / kind / runtime model
/// as sandboxes.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct SpaceCreateOptions {
    /// Image (default: the canonical Linux image).
    #[uniffi(default = None)]
    pub image: Option<String>,
    /// Where: `local` (free) or `cloud` (metered). `None`: the user default
    /// (`default.on`, `CUA_DEFAULT_ON`, else `local`).
    #[uniffi(default = None)]
    pub on: Option<String>,
    /// `auto` (default), `container` or `vm`.
    #[uniffi(default = None)]
    pub kind: Option<String>,
    /// `auto` (default) or an engine the location offers for the kind.
    #[uniffi(default = None)]
    pub runtime: Option<String>,
    /// Name (default `space-<hex>`).
    #[uniffi(default = None)]
    pub name: Option<String>,
    /// vCPUs. Local default 2; cloud 1-64 (most accounts run 1-8; Fleet
    /// decides), default the pool template's (4).
    #[uniffi(default = None)]
    pub cpus: Option<u32>,
    /// Memory in MiB. Local default 4096; cloud 512 up to 524288 (most
    /// accounts run 1024-32768; Fleet decides), default the pool
    /// template's (4096).
    #[uniffi(default = None)]
    pub memory_mb: Option<u64>,
    /// Grow the VM's disk to this many GiB (local VMs; default: the
    /// image's size, never smaller). Lume grows the macOS APFS container
    /// before the first boot; Linux images grow their root partition at
    /// boot. Containers and cloud Spaces refuse it.
    #[uniffi(default = None)]
    pub disk_gb: Option<u32>,
    /// Readiness budget (local; default 600 s).
    #[uniffi(default = None)]
    pub timeout_ms: Option<u64>,
    /// Wait until ready (default true).
    #[uniffi(default = None)]
    pub wait: Option<bool>,
    /// Return a reachable registered Space in the same location with the
    /// requested services instead of creating one.
    #[uniffi(default = false)]
    pub reuse: bool,
    /// Entrypoint override.
    #[uniffi(default = None)]
    pub command: Option<Vec<String>>,
    /// Guest environment.
    #[uniffi(default)]
    pub env: HashMap<String, String>,
    /// Named services (name → guest port), for example `{"mcp": 8765}`.
    #[uniffi(default)]
    pub services: HashMap<String, u16>,
    /// Whether the image runs cua-spacesd (default: yes for the canonical
    /// images, no for any other image).
    #[uniffi(default = None)]
    pub spacesd: Option<bool>,
    /// A GPU option of the runtime the Space runs on
    /// ([`Spaces::gpu_support`]): `paravirtual` for a macOS VM on Lume (GPU
    /// acceleration, experimental), `virgl` (QEMU on Linux), `nvidia` (a
    /// runc container on Linux); `auto` picks the runtime's own. `None`
    /// (the default): no GPU. Kept with the Space.
    #[uniffi(default = None)]
    pub gpu: Option<String>,
    /// Your own key for this create (an app's pending row), for
    /// [`Spaces::cancel_create`] before the Space's id is known.
    #[uniffi(default = None)]
    pub create_id: Option<String>,
}

/// One GPU option of a runtime (see [`Spaces::gpu_support`]).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct GpuOption {
    /// What to pass as `gpu`.
    pub id: String,
    /// How a person names it ("GPU acceleration").
    pub label: String,
    /// Experimental.
    pub experimental: bool,
    /// Works on this host now.
    pub supported: bool,
    /// Why not, one short line (empty when supported).
    pub reason: String,
    /// A page that explains it.
    pub learn_more: Option<String>,
    /// Estimated cost per hour while it runs (cloud GPU types).
    pub usd_per_hour: Option<f64>,
}

/// One limit of a host that provides Spaces and how much of it is used.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpacesHostLimit {
    /// `spaces` (every Space it provides) or `macos_vms`.
    pub resource: String,
    /// In use now.
    pub used: u32,
    /// The limit (0: none).
    pub limit: u32,
    /// Why the limit exists, for people.
    pub reason: String,
}

/// One of your machines that provides Spaces ([`Spaces::hosts`]).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpacesHost {
    /// What `on="host:<id>"` takes: the relay machine id, or the name a
    /// direct host was added as.
    pub id: String,
    /// Its name.
    pub name: String,
    /// `relay` or `direct`.
    pub via: String,
    /// It answered.
    pub online: bool,
    /// Its operating system (`macos`, `linux`, `windows`), when it answered.
    pub os: String,
    /// Its limits, when it answered.
    pub limits: Vec<SpacesHostLimit>,
}

impl From<cua_spaces::host_spaces::HostOffer> for SpacesHost {
    fn from(h: cua_spaces::host_spaces::HostOffer) -> Self {
        Self {
            id: h.id,
            name: h.name,
            via: h.via,
            online: h.online,
            os: h.os,
            limits: h
                .limits
                .into_iter()
                .map(|l| SpacesHostLimit {
                    resource: l.resource,
                    used: l.used,
                    limit: l.limit,
                    reason: l.reason,
                })
                .collect(),
        }
    }
}

impl From<dpb::SpacesHost> for SpacesHost {
    fn from(h: dpb::SpacesHost) -> Self {
        Self {
            id: h.id,
            name: h.name,
            via: h.via,
            online: h.online,
            os: h.os,
            limits: h
                .limits
                .into_iter()
                .map(|l| SpacesHostLimit {
                    resource: l.resource,
                    used: l.used,
                    limit: l.limit,
                    reason: l.reason,
                })
                .collect(),
        }
    }
}

/// The GPU options of one runtime: empty, with the reason, when it has
/// none.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct GpuSupport {
    /// `lume`, `qemu`, `container`, `gvisor`, `fleet`, a provider's name.
    pub runtime: String,
    /// Its options.
    pub options: Vec<GpuOption>,
    /// Why it has none.
    pub reason: String,
}

impl From<cua_sandbox_core::gpu::GpuSupport> for GpuSupport {
    fn from(g: cua_sandbox_core::gpu::GpuSupport) -> Self {
        Self {
            runtime: g.runtime,
            options: g
                .options
                .into_iter()
                .map(|o| GpuOption {
                    id: o.id,
                    label: o.label,
                    experimental: o.experimental,
                    supported: o.supported,
                    reason: o.reason,
                    learn_more: o.learn_more,
                    usd_per_hour: o.usd_per_hour,
                })
                .collect(),
            reason: g.reason,
        }
    }
}

impl From<dpb::GpuSupport> for GpuSupport {
    fn from(g: dpb::GpuSupport) -> Self {
        Self {
            runtime: g.runtime,
            options: g
                .options
                .into_iter()
                .map(|o| GpuOption {
                    id: o.id,
                    label: o.label,
                    experimental: o.experimental,
                    supported: o.supported,
                    reason: o.reason,
                    learn_more: (!o.learn_more.is_empty()).then_some(o.learn_more),
                    usd_per_hour: o.usd_per_hour,
                })
                .collect(),
            reason: g.reason,
        }
    }
}

/// What [`Spaces::cancel_create`] did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceCancelOutcome {
    /// The Space id the create had (`local:<name>`), when known.
    pub id: String,
    /// `cancelled`, `not_creating` (nothing by that key is running) or
    /// `already_created` (it finished: delete it instead).
    pub state: String,
    /// What was removed and what stays, for people.
    pub message: String,
}

/// Result of [`Spaces::create`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceCreateResult {
    /// The Space, once ready.
    pub space: Option<SpaceInfo>,
    /// With `wait = false`: the id the Space will have.
    pub pending_id: Option<String>,
    /// `reuse` returned an existing Space.
    pub reused: bool,
}

/// What a Space create is doing (see [`Spaces::create_with_progress`]).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SpaceCreateProgress {
    /// `preparing`, `pulling`, `creating`, `booting`,
    /// `waiting_for_services`, `connecting` or `ready`, in that order.
    pub phase: String,
    /// How far through the phase, 0.0 to 1.0, when its size is known (an
    /// image pull).
    pub fraction: Option<f64>,
    /// One short line, for example the image being pulled.
    pub detail: String,
    /// Bytes downloaded so far, when the step counts them (an image pull).
    #[uniffi(default = None)]
    pub bytes_done: Option<u64>,
    /// Bytes the download has in all.
    #[uniffi(default = None)]
    pub bytes_total: Option<u64>,
    /// Smoothed download rate, bytes per second (time left: the bytes to
    /// go over it).
    #[uniffi(default = None)]
    pub bytes_per_second: Option<f64>,
    /// The id the Space will have (`local:<name>`), for
    /// [`Spaces::cancel_create`].
    #[uniffi(default = "")]
    pub space: String,
}

impl From<&cua_spaces::CreateProgress> for SpaceCreateProgress {
    fn from(p: &cua_spaces::CreateProgress) -> Self {
        Self {
            phase: p.phase.as_str().into(),
            fraction: p.fraction,
            detail: p.detail.clone(),
            bytes_done: p.bytes.map(|b| b.done),
            bytes_total: p.bytes.map(|b| b.total).filter(|t| *t > 0),
            bytes_per_second: p.bytes.and_then(|b| b.per_second),
            space: p.target.clone(),
        }
    }
}

/// Receives [`SpaceCreateProgress`] while [`Spaces::create_with_progress`]
/// runs, in order, ending with `ready`. Called on an SDK worker thread;
/// return quickly.
#[uniffi::export(with_foreign)]
pub trait SpaceCreateListener: Send + Sync {
    /// The create moved on.
    fn on_progress(&self, progress: SpaceCreateProgress);
}

/// Output of [`Space::bash`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceBashResult {
    /// Stdout.
    pub stdout: String,
    /// Stderr.
    pub stderr: String,
    /// Exit code (absent when signalled or timed out).
    pub exit_code: Option<i32>,
    /// Signal name, when killed by one.
    pub signal: Option<String>,
    /// Hit the timeout.
    pub timed_out: bool,
    /// Launch error.
    pub error: Option<String>,
    /// The `space_bash` tool text (`stdout`, `[stderr]`, `[exit N]`).
    pub rendered: String,
}

/// Result of [`Space::write`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct SpaceWriteReport {
    /// Guest path.
    pub path: String,
    /// Bytes.
    pub bytes: u64,
    /// SHA-256 verified in the Space.
    pub sha256: String,
}

/// Result of [`Space::upload`] / [`Space::download`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct SpaceTransferReport {
    /// Source.
    pub source: String,
    /// Destination.
    pub dest: String,
    /// `file` or `folder`.
    pub kind: String,
    /// Files written.
    pub files: u64,
    /// Directories created (uploads).
    #[serde(default)]
    pub directories: u64,
    /// Total bytes.
    pub bytes: u64,
    /// SHA-256 (single files).
    #[serde(default)]
    pub sha256: Option<String>,
    /// Every file verified by SHA-256.
    pub verified: bool,
}

/// Options for [`Space::send_file`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceSendFileOptions {
    /// Downloads subdirectory (`None` = `~/Downloads`), or a `~/Downloads/…`
    /// path.
    #[uniffi(default = None)]
    pub target_directory: Option<String>,
    /// Honor ignore files when sending a folder.
    #[uniffi(default = true)]
    pub respect_ignore_files: bool,
    /// When a file of the same name is already there: `rename` (default,
    /// keeps both as `name (1).ext`), `skip`, or `overwrite` to replace it.
    #[uniffi(default = None)]
    pub conflict: Option<String>,
}

impl Default for SpaceSendFileOptions {
    fn default() -> Self {
        Self {
            target_directory: None,
            respect_ignore_files: true,
            conflict: None,
        }
    }
}

/// One file placed by [`Space::send_file`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct SpaceSentFile {
    /// Guest path.
    pub path: String,
    /// Bytes.
    pub size: u64,
    /// SHA-256, verified on both ends.
    pub sha256: String,
}

/// Result of [`Space::send_file`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct SpaceSendFileReport {
    /// Host source.
    pub source: String,
    /// Transfer root in the guest.
    pub destination: String,
    /// Where the file or folder landed.
    pub dest: String,
    /// `file` or `folder`.
    pub kind: String,
    /// Files placed.
    pub files: Vec<SpaceSentFile>,
    /// Total bytes.
    pub bytes: u64,
    /// Paths left out by ignore rules (first 200).
    pub skipped_by_ignorefiles: Vec<String>,
    /// Paths the guest skipped.
    pub skipped_by_guest: Vec<String>,
    /// Every hash matched.
    pub verified: bool,
}

/// A tool of a Space's MCP service.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceToolInfo {
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Input JSON Schema.
    pub input_schema_json: String,
    /// Read-only.
    pub read_only: bool,
    /// Destructive.
    pub destructive: bool,
}

/// The result of a tool call (MCP content as JSON).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceToolResult {
    /// MCP content parts as a JSON array.
    pub content_json: String,
    /// Structured content as JSON, when present.
    pub structured_json: Option<String>,
    /// The tool failed.
    pub is_error: bool,
    /// Concatenated text parts.
    pub text: String,
    /// The result's `_meta` as JSON, when present.
    pub meta_json: Option<String>,
}

impl SpaceToolResult {
    fn with_meta(
        content: Vec<Value>,
        structured: Option<Value>,
        is_error: bool,
        meta: Option<Value>,
    ) -> Self {
        let text = content
            .iter()
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        SpaceToolResult {
            content_json: Value::Array(content).to_string(),
            structured_json: structured.map(|s| s.to_string()),
            is_error,
            text,
            meta_json: meta.map(|m| m.to_string()),
        }
    }
}

/// A streamable window.
#[derive(Debug, Clone, PartialEq, uniffi::Record, Deserialize)]
pub struct SpaceWindow {
    /// Window handle.
    pub window_id: String,
    /// Generation.
    pub epoch: u64,
    /// Title.
    pub title: String,
    /// App name.
    pub app_name: String,
    /// App id.
    pub app_id: String,
    /// Pid.
    pub pid: u32,
    /// `[x, y, width, height]` in logical points.
    pub bounds: Vec<f64>,
    /// On screen.
    pub on_screen: bool,
    /// Focused.
    pub focused: bool,
    /// Streamable now.
    pub available: bool,
    /// Why not.
    #[serde(default)]
    pub limitation: String,
}

/// Memory and storage use ([`Space::usage`]). A size is the guest's own
/// limit only when its `*_limited` flag says so (a VM's memory or disk, a
/// container's cgroup memory limit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct SpaceUsage {
    /// Memory in use, bytes.
    pub memory_used: u64,
    /// Memory size, bytes.
    pub memory_total: u64,
    /// `memory_total` is the guest's limit.
    pub memory_limited: bool,
    /// Home filesystem bytes in use.
    pub disk_used: u64,
    /// Home filesystem size, bytes.
    pub disk_total: u64,
    /// `disk_total` is the guest's own disk.
    pub disk_limited: bool,
}

/// One of a Space's displays ([`Space::displays`]).
#[derive(Debug, Clone, PartialEq, uniffi::Record, Deserialize)]
pub struct SpaceDisplay {
    /// Display id.
    pub id: String,
    /// Name ("XVFB-0", "Built-in Retina Display").
    pub name: String,
    /// The primary display.
    pub primary: bool,
    /// Framebuffer width in physical pixels.
    pub width_px: u32,
    /// Framebuffer height in physical pixels.
    pub height_px: u32,
    /// Physical pixels per logical point.
    pub scale_factor: f64,
}

/// An app icon from [`Space::app_icons`]: the icon the guest desktop uses,
/// normalized by the SDK's icon cache.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceAppIcon {
    /// The 64 px PNG (2x), or an SVG the guest could not rasterize.
    pub bytes: Vec<u8>,
    /// `image/png` or `image/svg+xml`.
    pub content_type: String,
    /// The 32 px PNG (1x); empty for an SVG.
    pub bytes_1x: Vec<u8>,
}

impl From<cua_spaces::AppIcon> for SpaceAppIcon {
    fn from(i: cua_spaces::AppIcon) -> Self {
        SpaceAppIcon {
            bytes: i.bytes,
            content_type: i.content_type.into(),
            bytes_1x: i.bytes_1x,
        }
    }
}

/// One window's app, for [`Space::app_icons`].
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct SpaceAppIconRequest {
    /// App name, as the window list reports it.
    pub app_name: String,
    /// App id (bundle id, `.desktop` id).
    #[uniffi(default = "")]
    pub app_id: String,
    /// A process of the app (0 when unknown).
    #[uniffi(default = 0)]
    pub pid: u32,
}

/// What to stream and how.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct SpaceStreamOptions {
    /// Window handle (wins over `app_name` and `display`).
    #[uniffi(default = None)]
    pub window_id: Option<String>,
    /// The largest window of this app.
    #[uniffi(default = None)]
    pub app_name: Option<String>,
    /// Display id (`None` = primary).
    #[uniffi(default = None)]
    pub display: Option<String>,
    /// Codecs in preference order (`h264`, `bgra`, `png`; empty = any).
    #[uniffi(default = [])]
    pub codecs: Vec<String>,
    /// FPS cap (0 = 30).
    #[uniffi(default = 0)]
    pub max_fps: u32,
    /// Long-edge cap (0 = native).
    #[uniffi(default = 0)]
    pub max_dimension: u32,
    /// Request the paired audio track.
    #[uniffi(default = false)]
    pub audio: bool,
    /// Ticket lifetime (default 60 s).
    #[uniffi(default = None)]
    pub ticket_ttl_ms: Option<u64>,
    /// Input policy: `view_only` (default), `background_only` (input
    /// without activating the target) or `allow_activation`.
    #[uniffi(default = None)]
    pub policy: Option<String>,
}

/// A minted media session.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceStreamTicket {
    /// Space id.
    pub space: String,
    /// Media session id.
    pub media_session_id: String,
    /// Media WebSocket URL, ticket included (the daemon passthrough in
    /// daemon mode).
    pub ws_url: String,
    /// The bare ticket.
    pub ticket: String,
    /// Expiry (RFC 3339).
    pub ticket_expires_at: Option<String>,
    /// `h264`, `bgra` or `png`.
    pub codec: String,
    /// Media wire version.
    pub wire_version: u32,
    /// Initial width.
    pub width: u32,
    /// Initial height.
    pub height: u32,
    /// Attaching needs [`Space::websocket_headers`] (Fleet gateway or the
    /// daemon passthrough).
    pub needs_headers: bool,
}

/// Delivery counters of a [`SpaceStreamSession`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct SpaceStreamStats {
    /// Frames delivered.
    pub frames: u64,
    /// Keyframes delivered.
    pub keyframes: u64,
    /// Frames dropped (sink behind; each forces a keyframe resync).
    pub frames_dropped: u64,
    /// Frames held back awaiting a keyframe.
    pub frames_gated: u64,
    /// Keyframe requests sent.
    pub keyframe_requests: u64,
    /// Audio packets delivered.
    pub audio_packets: u64,
    /// Audio packets reported lost.
    pub audio_lost: u64,
    /// Control events delivered.
    pub events: u64,
    /// Malformed messages ignored.
    pub malformed: u64,
}

impl From<cua_spaces::stream::StreamStats> for SpaceStreamStats {
    fn from(s: cua_spaces::stream::StreamStats) -> Self {
        SpaceStreamStats {
            frames: s.frames,
            keyframes: s.keyframes,
            frames_dropped: s.frames_dropped,
            frames_gated: s.frames_gated,
            keyframe_requests: s.keyframe_requests,
            audio_packets: s.audio_packets,
            audio_lost: s.audio_lost,
            events: s.events,
            malformed: s.malformed,
        }
    }
}

/// The stable presence color of an agent or other principal (`#rrggbb`): a
/// cursor palette color picked by a hash of `id`. Request it when the agent
/// joins presence (`PresenceIdentity.color`) and use it as the agent's
/// avatar background, so both come from one source.
#[uniffi::export]
pub fn presence_color(id: String) -> String {
    cua_spaces::presence::color_for(&id).to_string()
}

/// Black or white text (`#000000` / `#ffffff`), whichever reads better on
/// `background` (`#rrggbb`).
#[uniffi::export]
pub fn presence_text_color(background: String) -> String {
    cua_spaces::presence::text_color_on(&background).to_string()
}

/// Who joins presence.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct PresenceIdentity {
    /// Stable principal id.
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Requested color.
    #[uniffi(default = "")]
    pub color: String,
    /// An agent rather than a human.
    #[uniffi(default = false)]
    pub agent: bool,
}

/// A presence participant.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PresenceParticipant {
    /// Participant id (per join).
    pub participant_id: String,
    /// Principal id.
    pub principal_id: String,
    /// Display name.
    pub display_name: String,
    /// Color.
    pub color: String,
    /// `human` or `agent`.
    pub kind: String,
}

impl From<cua_spaces::presence::Participant> for PresenceParticipant {
    fn from(p: cua_spaces::presence::Participant) -> Self {
        PresenceParticipant {
            participant_id: p.participant_id,
            principal_id: p.principal_id,
            display_name: p.display_name,
            color: p.color,
            kind: p.kind,
        }
    }
}

/// A normalized cursor.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PresenceCursor {
    /// Display id (empty = primary).
    #[uniffi(default = "")]
    pub display_id: String,
    /// Window handle.
    #[uniffi(default = None)]
    pub window_id: Option<String>,
    /// X in `[0, 1]`.
    pub x: f64,
    /// Y in `[0, 1]`.
    pub y: f64,
    /// Visible.
    #[uniffi(default = true)]
    pub visible: bool,
    /// A button is down (datagram channel only).
    #[uniffi(default = false)]
    pub pressed: bool,
    /// The guest's cursor shape here (server-computed): `arrow`, `text`,
    /// `pointer`, `resize_ns`, `resize_ew`, `resize_nesw`, `resize_nwse`,
    /// `wait`, `progress`, `not_allowed`, `crosshair`, `grab`, `grabbing`,
    /// `move`. Ignored when publishing.
    #[uniffi(default = "arrow")]
    pub shape: String,
    /// How `shape` was determined: `unspecified`, `hit_test`, `system` or
    /// `probe`. Ignored when publishing.
    #[uniffi(default = "unspecified")]
    pub shape_source: String,
    /// Server time of the sample in milliseconds (0 = unknown).
    #[uniffi(default = 0.0)]
    pub at_ms: f64,
    /// Local Unix time it was received in milliseconds (0 = unknown).
    #[uniffi(default = 0.0)]
    pub received_ms: f64,
}

impl From<cua_spaces::presence::Cursor> for PresenceCursor {
    fn from(c: cua_spaces::presence::Cursor) -> Self {
        PresenceCursor {
            display_id: c.display_id,
            window_id: c.window_id,
            x: c.x,
            y: c.y,
            visible: c.visible,
            pressed: c.pressed,
            shape: c.shape.as_str().into(),
            shape_source: c.shape_source.as_str().into(),
            at_ms: c.at_ms,
            received_ms: c.received_ms,
        }
    }
}

impl From<PresenceCursor> for cua_spaces::presence::Cursor {
    fn from(c: PresenceCursor) -> Self {
        cua_spaces::presence::Cursor {
            display_id: c.display_id,
            window_id: c.window_id,
            x: c.x,
            y: c.y,
            visible: c.visible,
            pressed: c.pressed,
            shape: cua_spaces::presence::CursorShape::parse(&c.shape),
            shape_source: cua_spaces::presence::ShapeSource::parse(&c.shape_source),
            at_ms: c.at_ms,
            received_ms: c.received_ms,
        }
    }
}

/// A roster entry.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PresenceMember {
    /// Who.
    pub participant: PresenceParticipant,
    /// Their cursor, if known.
    pub cursor: Option<PresenceCursor>,
}

/// A presence event.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PresenceEvent {
    /// `joined`, `left`, `cursor_moved`, `shape_changed`, `heartbeat` or
    /// `keep_alive`.
    pub kind: String,
    /// `joined`: who.
    pub participant: Option<PresenceParticipant>,
    /// `left` / `cursor_moved` / `shape_changed`: whose.
    pub participant_id: Option<String>,
    /// `cursor_moved`: where.
    pub cursor: Option<PresenceCursor>,
    /// `shape_changed`: the new shape.
    #[uniffi(default = None)]
    pub shape: Option<String>,
    /// `shape_changed`: how it was determined.
    #[uniffi(default = None)]
    pub shape_source: Option<String>,
    /// `left`: why (`left`, `disconnected`, `timeout`, `run_ended`, or
    /// empty).
    #[uniffi(default = None)]
    pub reason: Option<String>,
    /// `heartbeat`: everyone present.
    #[uniffi(default = None)]
    pub participant_ids: Option<Vec<String>>,
}

impl From<cua_spaces::presence::PresenceEvent> for PresenceEvent {
    fn from(e: cua_spaces::presence::PresenceEvent) -> Self {
        use cua_spaces::presence::PresenceEvent as E;
        let mut out = PresenceEvent {
            kind: String::new(),
            participant: None,
            participant_id: None,
            cursor: None,
            shape: None,
            shape_source: None,
            reason: None,
            participant_ids: None,
        };
        match e {
            E::Joined { participant } => {
                out.kind = "joined".into();
                out.participant = Some(participant.into());
            }
            E::Left {
                participant_id,
                reason,
            } => {
                out.kind = "left".into();
                out.participant_id = Some(participant_id);
                out.reason = Some(reason);
            }
            E::CursorMoved {
                participant_id,
                cursor,
            } => {
                out.kind = "cursor_moved".into();
                out.participant_id = Some(participant_id);
                out.cursor = Some(cursor.into());
            }
            E::ShapeChanged {
                participant_id,
                shape,
                source,
            } => {
                out.kind = "shape_changed".into();
                out.participant_id = Some(participant_id);
                out.shape = Some(shape.as_str().into());
                out.shape_source = Some(source.as_str().into());
            }
            E::Heartbeat { participant_ids } => {
                out.kind = "heartbeat".into();
                out.participant_ids = Some(participant_ids);
            }
            E::KeepAlive => out.kind = "keep_alive".into(),
        }
        out
    }
}

impl PresenceEvent {
    /// Back to the core event; `None` for an unknown kind or a missing field.
    fn to_core(&self) -> Option<cua_spaces::presence::PresenceEvent> {
        use cua_spaces::presence::{CursorShape, PresenceEvent as E, ShapeSource};
        let pid = || self.participant_id.clone();
        Some(match self.kind.as_str() {
            "joined" => E::Joined {
                participant: self.participant.clone()?.into(),
            },
            "left" => E::Left {
                participant_id: pid()?,
                reason: self.reason.clone().unwrap_or_default(),
            },
            "cursor_moved" => E::CursorMoved {
                participant_id: pid()?,
                cursor: self.cursor.clone()?.into(),
            },
            "shape_changed" => E::ShapeChanged {
                participant_id: pid()?,
                shape: CursorShape::parse(self.shape.as_deref()?),
                source: ShapeSource::parse(self.shape_source.as_deref().unwrap_or_default()),
            },
            "heartbeat" => E::Heartbeat {
                participant_ids: self.participant_ids.clone()?,
            },
            "keep_alive" => E::KeepAlive,
            _ => return None,
        })
    }
}

impl From<PresenceParticipant> for cua_spaces::presence::Participant {
    fn from(p: PresenceParticipant) -> Self {
        cua_spaces::presence::Participant {
            participant_id: p.participant_id,
            principal_id: p.principal_id,
            display_name: p.display_name,
            color: p.color,
            kind: p.kind,
        }
    }
}

/// A normalized point.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct PresencePoint {
    /// X in `[0, 1]`.
    pub x: f64,
    /// Y in `[0, 1]`.
    pub y: f64,
}

/// One cursor to draw now (see [`PresenceView::drawables`]).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct PresenceDrawable {
    /// Whose.
    pub participant_id: String,
    /// Their display name.
    pub display_name: String,
    /// Their color (`#rrggbb`).
    pub color: String,
    /// This client's own cursor, drawn at the local pointer.
    pub is_me: bool,
    /// An agent.
    pub is_agent: bool,
    /// Normalized x.
    pub x: f64,
    /// Normalized y.
    pub y: f64,
    /// Shape name (see [`PresenceCursor::shape`]).
    pub shape: String,
    /// How the shape was determined.
    pub shape_source: String,
    /// Opacity in `[0, 1]` (idle cursors fade).
    pub alpha: f64,
    /// Display id of the cursor's target.
    pub display_id: String,
    /// Window id, when over a window stream.
    pub window_id: Option<String>,
}

impl From<cua_spaces::presence::Drawable> for PresenceDrawable {
    fn from(d: cua_spaces::presence::Drawable) -> Self {
        PresenceDrawable {
            participant_id: d.participant_id,
            display_name: d.display_name,
            color: d.color,
            is_me: d.is_me,
            is_agent: d.is_agent,
            x: d.x,
            y: d.y,
            shape: d.shape.as_str().into(),
            shape_source: d.shape_source.as_str().into(),
            alpha: d.alpha,
            display_id: d.display_id,
            window_id: d.window_id,
        }
    }
}

/// The shared presence netcode model: who is present and what to draw,
/// with remote cursors interpolated (render delay, Catmull-Rom,
/// extrapolation cap), idle cursors faded, stale participants dropped and
/// your own cursor at the local pointer (libs/cua/proto/PRESENCE.md
/// section 4). Feed it every presence event; call `drawables` each frame.
#[derive(uniffi::Object)]
pub struct PresenceView {
    inner: std::sync::Mutex<cua_spaces::presence::PresenceView>,
}

impl PresenceView {
    fn wrap(v: cua_spaces::presence::PresenceView) -> Arc<Self> {
        Arc::new(PresenceView {
            inner: std::sync::Mutex::new(v),
        })
    }
}

#[uniffi::export]
impl PresenceView {
    /// A view for `me` (the caller's participant id), rendering remote
    /// cursors `delay_ms` behind (default 100; 66 on the datagram channel).
    #[uniffi::constructor(default(delay_ms = None))]
    pub fn new(me: String, delay_ms: Option<f64>) -> Arc<Self> {
        Self::wrap(cua_spaces::presence::PresenceView::new(
            &me,
            delay_ms.unwrap_or(cua_spaces::presence::view::STREAM_DELAY_MS),
        ))
    }

    /// Adds or updates a participant.
    pub fn upsert(&self, participant: PresenceParticipant) {
        self.inner.lock().unwrap().upsert(participant.into());
    }

    /// Folds one event in at `local_ms` (see [`presence_now_ms`]). Returns
    /// whether anything changed.
    pub fn apply(&self, event: PresenceEvent, local_ms: f64) -> bool {
        match event.to_core() {
            Some(e) => self.inner.lock().unwrap().apply(&e, local_ms),
            None => false,
        }
    }

    /// Everything to draw at `local_ms`, your own cursor at `pointer` when
    /// the local pointer is over the surface.
    #[uniffi::method(default(pointer = None))]
    pub fn drawables(
        &self,
        local_ms: f64,
        pointer: Option<PresencePoint>,
    ) -> Vec<PresenceDrawable> {
        self.inner
            .lock()
            .unwrap()
            .drawables(local_ms, pointer.map(|p| (p.x, p.y)))
            .into_iter()
            .map(Into::into)
            .collect()
    }

    /// Drops everyone but the caller when heartbeats stopped for 3
    /// intervals. Returns the removed ids.
    pub fn expire(&self, local_ms: f64) -> Vec<String> {
        self.inner.lock().unwrap().expire(local_ms)
    }

    /// Participant ids, the caller first.
    pub fn participant_ids(&self) -> Vec<String> {
        self.inner.lock().unwrap().participant_ids()
    }

    /// A participant.
    pub fn participant(&self, participant_id: String) -> Option<PresenceParticipant> {
        self.inner
            .lock()
            .unwrap()
            .participant(&participant_id)
            .cloned()
            .map(Into::into)
    }

    /// A participant's current shape name (the caller's included).
    pub fn shape_of(&self, participant_id: String) -> Option<String> {
        self.inner
            .lock()
            .unwrap()
            .shape_of(&participant_id)
            .map(|(s, _)| s.as_str().to_string())
    }

    /// The expected heartbeat interval (default 5000 ms).
    pub fn set_heartbeat_interval_ms(&self, ms: f64) {
        self.inner.lock().unwrap().set_heartbeat_interval_ms(ms);
    }
}

/// The local clock [`PresenceView`] expects: Unix time in milliseconds.
#[uniffi::export]
pub fn presence_now_ms() -> f64 {
    cua_spaces::presence::now_ms()
}

/// One cursor shape's shared art: an SVG path on a square canvas, filled
/// with the participant's color over an outline.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CursorArt {
    /// Shape name.
    pub shape: String,
    /// SVG path data (absolute `M`, `L`, `C`, `Z`; nonzero fill).
    pub path_d: String,
    /// Hot spot x on the canvas.
    pub hotspot_x: f64,
    /// Hot spot y on the canvas.
    pub hotspot_y: f64,
    /// Canvas size (square).
    pub canvas: f64,
    /// Outline color, drawn under the fill.
    pub outline_color: String,
    /// Outline width in canvas units.
    pub outline_width: f64,
}

impl From<&cua_spaces::presence::art::CursorArt> for CursorArt {
    fn from(a: &cua_spaces::presence::art::CursorArt) -> Self {
        CursorArt {
            shape: a.shape.as_str().into(),
            path_d: a.path_d.clone(),
            hotspot_x: a.hotspot_x,
            hotspot_y: a.hotspot_y,
            canvas: a.canvas,
            outline_color: a.outline_color.clone(),
            outline_width: a.outline_width,
        }
    }
}

/// The shared art for `shape` (unknown names give the arrow).
#[uniffi::export]
pub fn presence_cursor_art(shape: String) -> CursorArt {
    cua_spaces::presence::art::cursor_art(cua_spaces::presence::CursorShape::parse(&shape)).into()
}

/// The shared art for every shape.
#[uniffi::export]
pub fn presence_cursor_art_all() -> Vec<CursorArt> {
    cua_spaces::presence::art::all_cursor_art()
        .iter()
        .map(Into::into)
        .collect()
}

/// A standalone SVG of `shape` in `color`, `size` pixels square.
#[uniffi::export]
pub fn presence_cursor_art_svg(shape: String, color: String, size: f64) -> String {
    cua_spaces::presence::art::cursor_art_svg(
        cua_spaces::presence::CursorShape::parse(&shape),
        &color,
        size,
    )
}

/// The presence conformance vectors (JSON), for binding test suites.
#[uniffi::export]
pub fn presence_conformance_json() -> String {
    cua_spaces::presence::view::CONFORMANCE_JSON.to_string()
}

/// A human's answer to a teleport manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct TeleportDecision {
    /// Items to send (`None` = the provider's default-checked set, never
    /// everything). An empty list is refused as ambiguous.
    #[uniffi(default = None)]
    pub include: Option<Vec<String>>,
    /// The human acknowledged the sensitive items in the selection.
    #[uniffi(default = false)]
    pub acknowledge_sensitive: bool,
}

/// What a teleport moved, or how to retry a pending Keyvault approval.
///
/// Through a daemon (`Host::Daemon`), the underlying `teleport_app` tool is
/// consent-gated: a first call never delivers, it only files the request and
/// returns `status: "pending"` with a `request_id`; show the user the
/// Keyvault approval step, then call [`Space::teleport`] again with that
/// same `request_id` (`TeleportRetry::request_id`). A second `"pending"`
/// means the user has not decided yet -- call again the same way. Only
/// `status: "moved"` means the session actually landed. An embedded host
/// (no separate daemon) always returns `"moved"` in one call: there is
/// nothing to retry.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct TeleportReceipt {
    /// `moved` or `pending`.
    #[serde(default = "status_moved")]
    pub status: String,
    /// The Keyvault request to retry with, while pending.
    #[serde(default)]
    pub request_id: Option<String>,
    /// App.
    #[serde(default)]
    pub app: String,
    /// Space.
    #[serde(default)]
    pub space: String,
    /// `import_session`.
    #[serde(default)]
    pub method: String,
    /// Paths sent.
    #[serde(default)]
    pub transferred_paths: Vec<String>,
    /// Bundle bytes.
    #[serde(default)]
    pub bundle_bytes: u64,
    /// Bundle SHA-256, verified by the Space.
    #[serde(default)]
    pub bundle_sha256: String,
    /// Groups imported.
    #[serde(default)]
    pub imported: Vec<String>,
    /// Items skipped, with reasons.
    #[serde(default)]
    pub skipped: Vec<String>,
    /// The app was launched.
    #[serde(default)]
    pub launched: bool,
}

fn status_moved() -> String {
    "moved".to_string()
}

/// The consent gate of [`Space::teleport`]: shown the manifest, returns
/// the human's decision, or `None` to cancel. Runs on a blocking worker
/// thread; it may take as long as the human needs.
#[uniffi::export(with_foreign)]
pub trait TeleportApprover: Send + Sync {
    /// Decides.
    fn approve(&self, manifest: TeleportManifest) -> Option<TeleportDecision>;
}

/// Hotspot state.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct SpaceHotspotStatus {
    /// Space.
    pub space: String,
    /// `stopped`, `waiting_for_peer` or `active`.
    pub state: String,
    /// Hotspot id.
    pub hotspot_id: String,
    /// Guest SOCKS address.
    pub socks_address: String,
    /// Open relayed connections.
    pub active_connections: u32,
    /// Bytes out of the guest.
    pub bytes_out: u64,
    /// Bytes into the guest.
    pub bytes_in: u64,
    /// Served by the process that owns the Spaces runtime.
    pub served_here: bool,
}

/// One account a Space is shared with.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct SpaceShareEntry {
    /// The email or account id, as shared.
    pub who: String,
    /// `viewer` or `editor`.
    pub role: String,
    /// Connected through the relay right now.
    #[serde(default)]
    pub connected: bool,
}

/// Who a Space is shared with.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct SpaceShares {
    /// The Space.
    pub space: String,
    /// The relay machine it is shared as (empty when not shared).
    #[serde(default)]
    pub machine: String,
    /// What the people it is shared with open (`relay:<machine>`).
    #[serde(default)]
    pub invitee_space: String,
    /// The relay URL of the machine.
    #[serde(default)]
    pub url: String,
    /// The machine is on the relay now.
    #[serde(default)]
    pub online: bool,
    /// The accounts and their roles.
    #[serde(default)]
    pub shares: Vec<SpaceShareEntry>,
}

/// A Space published on the relay ([`Spaces::relay_register`]).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct SpaceRelayRegistration {
    /// The Space.
    pub space: String,
    /// How the account's other devices reach it: `relay:<machine>`.
    pub relay_space: String,
    /// The relay machine id.
    pub machine: String,
    /// The relay URL of the machine.
    #[serde(default)]
    pub url: String,
    /// Connected to the relay now.
    #[serde(default)]
    pub online: bool,
}

/// What [`Spaces::stop`] or [`Spaces::start`] did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct SpacePowerReport {
    /// The Space.
    pub space: String,
    /// `running`, `suspended` or `stopped`: the state it is in now.
    pub state: String,
    /// How it turns off: `suspend` (its memory is kept) or `stop` (its
    /// disk is kept).
    pub power: String,
    /// What happened, for people.
    #[serde(default)]
    pub message: String,
}

/// Result of [`Space::agent_start`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentStartReport {
    /// Run id.
    pub run_id: String,
    /// Agent.
    pub agent: String,
    /// Space.
    pub space: String,
    /// spacesd process tag of the first turn.
    pub process_tag: String,
    /// Preparation steps that could not be completed.
    pub notes: Vec<String>,
    /// The whole report as JSON.
    pub json: String,
}

/// One agent run's status.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentRunStatus {
    /// Run id.
    pub run_id: String,
    /// Agent.
    pub agent: Option<String>,
    /// Status (`running`, `done`, `failed`, ...).
    pub status: String,
    /// Why.
    pub reason: String,
    /// Finer grain: `installing`, `working`, `waiting`, `exited`, ...
    pub phase: String,
    /// The last turn's final text (`agent_status` only).
    pub result_text: Option<String>,
    /// A follow-up sent now starts the next turn (the server's rule: idle,
    /// or a crashed or failed run it restarts; not mid-turn). Read this,
    /// never derive it from `status`.
    pub accepts_message: bool,
    /// Output tail.
    pub output_tail: Option<String>,
    /// The whole status as JSON.
    pub json: String,
}

impl AgentRunStatus {
    fn from_json(v: Value) -> Result<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Ok(AgentRunStatus {
            run_id: s("run_id").unwrap_or_default(),
            agent: s("agent"),
            status: status_string(v.get("status")),
            reason: s("reason").unwrap_or_default(),
            phase: s("phase").unwrap_or_default(),
            result_text: v
                .get("result")
                .and_then(|r| r.get("text"))
                .and_then(Value::as_str)
                .map(str::to_string),
            accepts_message: v
                .get("accepts_message")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            output_tail: s("output_tail"),
            json: v.to_string(),
        })
    }
}

fn status_string(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// Options for [`Space::request_site_login`].
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct SiteLoginOptions {
    /// Which saved username, when the site has several.
    #[uniffi(default = None)]
    pub username: Option<String>,
    /// The persistent agent asking (shown to the user in the approval).
    #[uniffi(default = None)]
    pub agent: Option<String>,
    /// cua-driver lifecycle session of the tab to sign in.
    #[uniffi(default = None)]
    pub session: Option<String>,
    /// cua-driver browser target id (from `get_browser_state`).
    #[uniffi(default = None)]
    pub target_id: Option<String>,
    /// cua-driver tab id (from `get_browser_state`).
    #[uniffi(default = None)]
    pub tab_id: Option<String>,
    /// The request id a previous call returned (the retry after approval).
    #[uniffi(default = None)]
    pub request_id: Option<String>,
    /// Seconds to wait for the user's decision on a retry (default 20, at
    /// most 120).
    #[uniffi(default = None)]
    pub wait_secs: Option<u32>,
}

/// What [`Space::request_site_login`] did. Never the password.
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record, Deserialize)]
pub struct SiteLoginReport {
    /// `pending` (the user has not approved yet) or `filled`.
    pub status: String,
    /// The Keyvault request to retry with, while pending.
    #[serde(default)]
    pub request_id: Option<String>,
    /// Site of the saved login (`github.com`), once filled.
    #[serde(default)]
    pub site: Option<String>,
    /// The origin signed in to, once filled.
    #[serde(default)]
    pub origin: Option<String>,
    /// The username, masked (`a***@example.test`).
    #[serde(default)]
    pub username_hint: Option<String>,
    /// The form was submitted.
    #[serde(default)]
    pub submitted: bool,
    /// The tab's URL after the fill.
    #[serde(default)]
    pub page_url: Option<String>,
    /// The tab it signed in (cua-driver ids), to keep using it.
    #[serde(default)]
    pub session: Option<String>,
    /// See `session`.
    #[serde(default)]
    pub target_id: Option<String>,
    /// See `session`.
    #[serde(default)]
    pub tab_id: Option<String>,
}

/// Options for [`Space::agent_start`] beyond the harness and prompt.
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct SpaceAgentOptions {
    /// Provider key variables forwarded from the Spaces host's environment.
    #[uniffi(default)]
    pub env_from_host: Vec<String>,
    /// More environment for the agent, for example `HERMES_HOME` (stored
    /// 0600 in the run, redacted from its output; not for provider keys).
    #[uniffi(default)]
    pub env: std::collections::HashMap<String, String>,
    #[uniffi(default = None)]
    pub repo: Option<String>,
    #[uniffi(default = None)]
    pub branch: Option<String>,
    #[uniffi(default = None)]
    pub cwd: Option<String>,
    #[uniffi(default = None)]
    pub model: Option<String>,
    /// A custom model endpoint.
    #[uniffi(default = None)]
    pub base_url: Option<String>,
    /// Stop (resumably) once the prompt is answered.
    #[uniffi(default = None)]
    pub exit_when_idle: Option<bool>,
    /// Run as this persistent agent: its home in the Cua Volume is restored
    /// first and saved after every turn (see `Spaces.persistent_agent_send`).
    #[uniffi(default = None)]
    pub home: Option<String>,
}

/// Result of [`Space::agent_message`] / [`Space::agent_stop`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentActionReport {
    /// Run id.
    pub run_id: String,
    /// Delivered (message) or stopped (stop).
    pub ok: bool,
    /// Why.
    pub reason: String,
    /// The whole report as JSON.
    pub json: String,
}

// ------------------------------------------------------------ plumbing

fn tag_error(kind: &str, message: String) -> CuaError {
    match kind {
        "invalid_argument" => CuaError::InvalidArgument(message),
        "not_found" => CuaError::NotFound(message),
        "ambiguous_sandbox" => CuaError::AmbiguousSandbox(message),
        "insufficient_disk" => CuaError::InsufficientDisk(message),
        "spacesd_not_available" => CuaError::SpacesdNotAvailable(message),
        "capability_missing" => CuaError::CapabilityMissing(message),
        "host_capability_missing" => CuaError::HostCapabilityMissing(message),
        "wrong_provider" => CuaError::Unsupported(message),
        "teleport_refused" => CuaError::TeleportRefused(message),
        "timeout" => CuaError::Timeout(message),
        "cancelled" => CuaError::Cancelled(message),
        "unauthenticated" => CuaError::Unauthenticated(message),
        "fleet" => CuaError::Fleet(message),
        "fleet_admission_denied" => CuaError::FleetAdmissionDenied(message),
        "cloud_credit_exhausted" => CuaError::CloudCreditExhausted(message),
        "cloud" => CuaError::Cloud(message),
        "sandbox" => CuaError::Runtime(message),
        // Cua Volume: refusals by the access rules or a declined presence
        // prompt; a stale etag, a held lease or a blocked secret is a
        // problem with the input.
        "forbidden" | "not_confirmed" => CuaError::PermissionDenied(message),
        "precondition_failed" | "lease_held" | "secret_detected" => {
            CuaError::InvalidArgument(message)
        }
        "io" | "json" => CuaError::Internal(message),
        "mcp" => CuaError::Http(message),
        _ => CuaError::Env(message),
    }
}

impl From<cua_spaces::Error> for CuaError {
    fn from(e: cua_spaces::Error) -> Self {
        let kind = e.tag();
        let message = e.to_string();
        match e {
            cua_spaces::Error::Env(env) => env.into(),
            cua_spaces::Error::Fleet(f) => f.into(),
            _ => tag_error(kind, message),
        }
    }
}

fn daemon_err(s: tonic::Status) -> CuaError {
    cua_daemon::client::status_error(s).into()
}

fn outcome_value(out: ToolOut) -> Result<Value> {
    if out.is_error {
        let (kind, message) = out
            .structured
            .as_ref()
            .and_then(|s| s.get("error"))
            .map(|e| {
                (
                    e.get("kind").and_then(Value::as_str).unwrap_or("env"),
                    e.get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .unwrap_or(("env", out.text()));
        return Err(tag_error(kind, message));
    }
    let text = out.text();
    Ok(serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

pub(super) fn parse<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v)
        .map_err(|e| CuaError::Internal(format!("unexpected tool output: {e}")))
}

/// `Space::teleport` in process: the Cua Spaces teleport extension, when the
/// host registered it (the Spaces apps do), mints the approval from the
/// approver's decision and exports through its app-session providers.
/// Without it: `HostCapabilityMissing` (teleport ships with Cua Spaces).
async fn teleport_embedded(
    spaces: &cua_spaces::Spaces,
    space: &str,
    manifest: &TeleportManifest,
    decision: TeleportDecision,
) -> Result<TeleportReceipt> {
    let receipt = spaces
        .call_extension(
            "teleport",
            "teleport.send",
            json!({
                "space": space,
                "app": manifest.app,
                "scope": manifest.scope,
                "include": decision.include,
                "acknowledge_sensitive": decision.acknowledge_sensitive,
            }),
        )
        .await?;
    parse(receipt)
}

/// Turns the daemon's `teleport_app` answer into a [`TeleportReceipt`]:
/// `moved: true` is a completed teleport, `consent_required: true` (with or
/// without `status: "pending"`; the tool sends the former on the filing call
/// and both on a still-pending retry) is [`TeleportReceipt::status`]
/// `"pending"` with the `request_id` to retry with -- NOT an error, so a
/// caller that only checks `Result::Err` does not mistake "ask the user"
/// for "refused". Anything else (denied, malformed) is the refusal
/// [`keyvault_refusal`] already built.
fn teleport_app_response(v: &Value) -> Result<TeleportReceipt> {
    if v.get("moved").and_then(Value::as_bool) == Some(true) {
        let strings = |key: &str| -> Vec<String> {
            v.get(key)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };
        return Ok(TeleportReceipt {
            status: "moved".into(),
            request_id: None,
            app: v
                .get("app")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            space: v
                .get("space")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            method: "import_session".into(),
            transferred_paths: strings("transferred_paths"),
            bundle_bytes: 0,
            bundle_sha256: String::new(),
            imported: strings("transferred_paths"),
            skipped: Vec::new(),
            launched: false,
        });
    }
    if v.get("consent_required").and_then(Value::as_bool) == Some(true)
        && let Some(id) = v.get("request_id").and_then(Value::as_str)
    {
        return Ok(TeleportReceipt {
            status: "pending".into(),
            request_id: Some(id.to_string()),
            app: v
                .get("app")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            space: v
                .get("space")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            method: "import_session".into(),
            transferred_paths: Vec::new(),
            bundle_bytes: 0,
            bundle_sha256: String::new(),
            imported: Vec::new(),
            skipped: Vec::new(),
            launched: false,
        });
    }
    Err(keyvault_refusal(v))
}

/// The daemon's `teleport_app` answer as a typed refusal: the user declined,
/// or something else went wrong (never "pending" -- that is handled above
/// before this is reached).
fn keyvault_refusal(v: &Value) -> CuaError {
    let message = v
        .get("message")
        .or_else(|| v.pointer("/error/message"))
        .and_then(Value::as_str)
        .unwrap_or("moving a signed-in session needs the user's approval in Cua (Keyvault)");
    match v.get("request_id").and_then(Value::as_str) {
        Some(id) => CuaError::TeleportRefused(format!(
            "{message} Keyvault request {id}: approve it in Cua (Keyvault page)."
        )),
        None => CuaError::TeleportRefused(message.to_string()),
    }
}

fn via_json<T: serde::Serialize, U: serde::de::DeserializeOwned>(v: &T) -> Result<U> {
    parse(serde_json::to_value(v).map_err(|e| CuaError::Internal(e.to_string()))?)
}

struct ToolOut {
    content: Vec<Value>,
    structured: Option<Value>,
    is_error: bool,
    meta: Option<Value>,
}

impl ToolOut {
    fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Where the Spaces runtime lives.
#[derive(Clone)]
enum Host {
    Embedded(cua_spaces::Spaces),
    Daemon(DaemonClient),
}

impl Host {
    fn of(backend: &Backend) -> Self {
        match backend {
            Backend::Embedded(rt) => Host::Embedded(rt.spaces().clone()),
            Backend::Daemon(d) => Host::Daemon(d.clone()),
        }
    }

    async fn tool(&self, name: &str, arguments: Value) -> Result<ToolOut> {
        match self {
            Host::Embedded(spaces) => {
                let o = cua_spaces::mcp::McpServer::new(spaces.clone())
                    .call(name, arguments)
                    .await;
                Ok(ToolOut {
                    content: o.content,
                    structured: o.structured,
                    is_error: o.is_error,
                    meta: o.meta,
                })
            }
            Host::Daemon(d) => {
                let r = d
                    .spaces()
                    .call_space_tool(dpb::CallSpaceToolRequest {
                        name: name.into(),
                        arguments_json: arguments.to_string(),
                    })
                    .await
                    .map_err(daemon_err)?
                    .into_inner();
                let content: Vec<Value> = serde_json::from_str(&r.content_json)
                    .map_err(|e| CuaError::Internal(format!("content_json: {e}")))?;
                let structured = (!r.structured_json.is_empty())
                    .then(|| serde_json::from_str(&r.structured_json))
                    .transpose()
                    .map_err(|e| CuaError::Internal(format!("structured_json: {e}")))?;
                let meta = (!r.meta_json.is_empty())
                    .then(|| serde_json::from_str(&r.meta_json))
                    .transpose()
                    .map_err(|e| CuaError::Internal(format!("meta_json: {e}")))?;
                Ok(ToolOut {
                    content,
                    structured,
                    is_error: r.is_error,
                    meta,
                })
            }
        }
    }

    async fn tool_value(&self, name: &str, arguments: Value) -> Result<Value> {
        outcome_value(self.tool(name, arguments).await?)
    }

    async fn record(&self, space: &str) -> Result<SpaceInfo> {
        match self {
            Host::Embedded(s) => {
                let id = s.resolve(space)?;
                s.list()?
                    .into_iter()
                    .find(|i| i.id == id.to_string())
                    .map(Into::into)
                    .ok_or_else(|| CuaError::NotFound(format!("Space {id}")))
            }
            Host::Daemon(d) => d
                .spaces()
                .resolve_space(dpb::ResolveSpaceRequest {
                    space: space.into(),
                })
                .await
                .map_err(daemon_err)?
                .into_inner()
                .space
                .map(Into::into)
                .ok_or_else(|| CuaError::Internal("no space returned".into())),
        }
    }

    async fn connect(&self, space: &str) -> Result<(SpaceInfo, cua_spaces::Space)> {
        match self {
            Host::Embedded(s) => {
                let handle = s.space(space).await?;
                let info = self.record(&handle.id().to_string()).await?;
                Ok((info, handle))
            }
            Host::Daemon(d) => {
                let r = d
                    .spaces()
                    .connect_space(dpb::ConnectSpaceRequest {
                        space: space.into(),
                    })
                    .await
                    .map_err(daemon_err)?
                    .into_inner();
                let record = r
                    .space
                    .ok_or_else(|| CuaError::Internal("no space returned".into()))?;
                let caps =
                    <cua_spacesd_client::pb::GetCapabilitiesResponse as prost::Message>::decode(
                        r.capabilities.as_ref(),
                    )
                    .map_err(|e| CuaError::Internal(format!("capabilities: {e}")))?;
                let id = cua_spaces::SpaceId::parse(&record.id)?;
                let bearer = vec![("authorization".to_string(), format!("Bearer {}", r.token))];
                // Declared services through the daemon's service passthrough.
                let services = r
                    .services
                    .iter()
                    .map(|svc| {
                        (
                            svc.name.clone(),
                            cua_spaces::SpaceService {
                                source: cua_spaces::ServiceSource::Url(
                                    cua_sandbox_core::ServiceEndpoint {
                                        url: svc.url.clone(),
                                        headers: bearer.clone(),
                                    },
                                ),
                                mcp_path: svc.mcp_path.clone(),
                            },
                        )
                    })
                    .collect();
                if r.env_url.is_empty() {
                    let handle = cua_spaces::Space::generic(id, record.name.clone(), services);
                    return Ok((record.into(), handle));
                }
                let mut o = cua_spacesd_client::ConnectOptions::parse(&r.env_url)?
                    .transport(cua_spacesd_client::TransportPreference::Native)
                    .probe(false);
                o.token = Some(r.token.clone());
                let env = cua_spacesd_client::SpacesdClient::connect(o).await?;
                let handle = cua_spaces::Space::attach(id, record.name.clone(), env, caps, bearer)
                    .with_services(services);
                Ok((record.into(), handle))
            }
        }
    }
}

/// Calls Spaces tools where the runtime lives (for sibling modules).
#[derive(Clone)]
pub(super) struct SpacesToolCaller(Host);

impl SpacesToolCaller {
    /// The tool's JSON result, or its error.
    pub(super) async fn tool_value(&self, name: &str, arguments: Value) -> Result<Value> {
        self.0.tool_value(name, arguments).await
    }
}

// ------------------------------------------------------------ Spaces

/// The Spaces registry: add a machine by URL, claim a Fleet Space,
/// provision a local one, and get [`Space`] handles.
#[derive(uniffi::Object)]
pub struct Spaces {
    host: Host,
}

impl Spaces {
    pub(crate) fn new(backend: &Backend) -> Self {
        Self {
            host: Host::of(backend),
        }
    }

    /// An owned handle that calls this runtime's Spaces tools.
    pub(super) fn tool_caller(&self) -> SpacesToolCaller {
        SpacesToolCaller(self.host.clone())
    }
}

#[uniffi::export]
impl Spaces {
    /// Adds a Space by URL (`http(s)://host:port`, `host:port`, or a
    /// Space id such as `local:<name>`) after a `GetCapabilities` handshake. Any other MCP
    /// endpoint (`http://host:8765/mcp`) becomes a Space with one MCP
    /// service, `mcp`, and no spacesd capabilities.
    pub async fn add(
        &self,
        url: String,
        token: Option<String>,
        name: Option<String>,
    ) -> Result<SpaceInfo> {
        self.add_with_service(url, token, name, None).await
    }

    /// [`Spaces::add`], naming the service a plain MCP endpoint is
    /// registered under (default `mcp`).
    pub async fn add_with_service(
        &self,
        url: String,
        token: Option<String>,
        name: Option<String>,
        service: Option<String>,
    ) -> Result<SpaceInfo> {
        let host = self.host.clone();
        run(async move {
            match &host {
                Host::Embedded(s) => {
                    Ok(s.add_with_service(&url, token, name, service).await?.into())
                }
                Host::Daemon(d) => d
                    .spaces()
                    .add_space(dpb::AddSpaceRequest {
                        url,
                        token: token.unwrap_or_default(),
                        name: name.unwrap_or_default(),
                        service: service.unwrap_or_default(),
                    })
                    .await
                    .map_err(daemon_err)?
                    .into_inner()
                    .space
                    .map(Into::into)
                    .ok_or_else(|| CuaError::Internal("no space returned".into())),
            }
        })
        .await
    }

    /// Registered Spaces.
    pub async fn list(&self) -> Result<Vec<SpaceInfo>> {
        let host = self.host.clone();
        run(async move {
            match &host {
                Host::Embedded(s) => Ok(s.list()?.into_iter().map(Into::into).collect()),
                Host::Daemon(d) => Ok(d
                    .spaces()
                    .list_spaces(dpb::ListSpacesRequest {})
                    .await
                    .map_err(daemon_err)?
                    .into_inner()
                    .spaces
                    .into_iter()
                    .map(Into::into)
                    .collect()),
            }
        })
        .await
    }

    /// Resolves an id, legacy id, URL or display name.
    pub async fn resolve(&self, space: String) -> Result<SpaceInfo> {
        let host = self.host.clone();
        run(async move { host.record(&space).await }).await
    }

    /// Unregisters a Space (the sandbox is not touched).
    pub async fn remove(&self, space: String) -> Result<()> {
        let host = self.host.clone();
        run(async move {
            match &host {
                Host::Embedded(s) => {
                    s.remove(&space).await?;
                }
                Host::Daemon(d) => {
                    d.spaces()
                        .remove_space(dpb::RemoveSpaceRequest { id: space })
                        .await
                        .map_err(daemon_err)?;
                }
            }
            Ok(())
        })
        .await
    }

    /// Creates a Space where `options.on` says (the user default when
    /// unset): a new sandbox registered as a Space. An existing machine is
    /// added with [`Spaces::add`].
    pub async fn create(&self, options: SpaceCreateOptions) -> Result<SpaceCreateResult> {
        self.create_impl(options, None).await
    }

    /// [`Spaces::create`], reporting what it does to `listener` (pulling
    /// the image, booting, waiting for cua-spacesd, connecting) until the
    /// Space is ready. Always waits (`options.wait` is ignored). Through a
    /// daemon that predates progress, the listener hears only `ready`.
    pub async fn create_with_progress(
        &self,
        options: SpaceCreateOptions,
        listener: Arc<dyn SpaceCreateListener>,
    ) -> Result<SpaceCreateResult> {
        self.create_impl(
            SpaceCreateOptions {
                wait: Some(true),
                ..options
            },
            Some(listener),
        )
        .await
    }
}

impl Spaces {
    async fn create_impl(
        &self,
        options: SpaceCreateOptions,
        listener: Option<Arc<dyn SpaceCreateListener>>,
    ) -> Result<SpaceCreateResult> {
        use cua_sandbox_core::placement::{Kind, On, Runtime};
        let place =
            |e: cua_sandbox_core::placement::PlacementError| CuaError::InvalidPlacement(e.message);
        let text = |v: &Option<String>| {
            v.as_deref()
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
        };
        // The default location is resolved here, in the caller's process.
        let (on, _) = match text(&options.on) {
            Some(o) => (
                On::parse(&o).map_err(place)?,
                cua_sandbox_core::settings::Source::Explicit,
            ),
            None => cua_sandbox_core::settings::Settings::load()
                .and_then(|s| s.default_on())
                .map_err(|e| CuaError::InvalidArgument(e.to_string()))?,
        };
        let kind = Kind::parse(&text(&options.kind).unwrap_or_default()).map_err(place)?;
        let runtime = Runtime::parse(&text(&options.runtime).unwrap_or_default()).map_err(place)?;
        cua_sandbox_core::placement::validate(&on, kind, &runtime).map_err(place)?;
        let host = self.host.clone();
        run(async move {
            match &host {
                Host::Embedded(s) => {
                    let created = s
                        .create(cua_spaces::SpaceCreate {
                            image: options.image,
                            on: Some(on),
                            kind,
                            runtime,
                            name: options.name,
                            cpus: options.cpus,
                            memory_mb: options.memory_mb,
                            disk_gb: options.disk_gb,
                            timeout: options.timeout_ms.map(Duration::from_millis),
                            wait: options.wait,
                            reuse: options.reuse,
                            command: options.command,
                            env: options.env.into_iter().collect(),
                            services: options.services.into_iter().collect(),
                            spacesd: options.spacesd,
                            env_token: None,
                            progress: listener.clone().map(|l| {
                                cua_spaces::ProgressSink::new(move |p| l.on_progress(p.into()))
                            }),
                            create_id: options.create_id.clone(),
                            gpu: options.gpu.clone().filter(|g| !g.trim().is_empty()),
                        })
                        .await?;
                    Ok(match created {
                        cua_spaces::SpaceCreated::Ready { info, reused } => SpaceCreateResult {
                            space: Some(info.into()),
                            pending_id: None,
                            reused,
                        },
                        cua_spaces::SpaceCreated::Starting(p) => SpaceCreateResult {
                            space: None,
                            pending_id: Some(p.id),
                            reused: false,
                        },
                    })
                }
                Host::Daemon(d) => {
                    let request = dpb::CreateSpaceRequest {
                        image: options.image.unwrap_or_default(),
                        location: on.to_string(),
                        kind: kind.to_string(),
                        runtime: runtime.to_string(),
                        name: options.name.unwrap_or_default(),
                        cpus: options.cpus.unwrap_or(0),
                        memory_mb: options.memory_mb.unwrap_or(0),
                        disk_gb: options.disk_gb.unwrap_or(0),
                        timeout: options.timeout_ms.map(|ms| pbjson_types::Duration {
                            seconds: (ms / 1000) as i64,
                            nanos: ((ms % 1000) * 1_000_000) as i32,
                        }),
                        wait: options.wait,
                        reuse: options.reuse,
                        command: options.command.unwrap_or_default(),
                        env: options.env,
                        services: options
                            .services
                            .into_iter()
                            .map(|(k, v)| (k, u32::from(v)))
                            .collect(),
                        spacesd: options.spacesd,
                        gpu: options.gpu.unwrap_or_default(),
                        create_id: options.create_id.unwrap_or_default(),
                    };
                    let r = match listener {
                        Some(l) => daemon_create_streaming(d, request, l).await?,
                        None => d
                            .spaces()
                            .create_space(request)
                            .await
                            .map_err(daemon_err)?
                            .into_inner(),
                    };
                    Ok(if r.phase == "starting" {
                        SpaceCreateResult {
                            space: None,
                            pending_id: r.space.map(|s| s.id),
                            reused: false,
                        }
                    } else {
                        SpaceCreateResult {
                            space: r.space.map(Into::into),
                            pending_id: None,
                            reused: r.reused,
                        }
                    })
                }
            }
        })
        .await
    }
}

/// How long a create stream may stay silent (see [`daemon_create_streaming`]).
const CREATE_STREAM_IDLE: Duration = Duration::from_secs(15 * 60);
/// Over a caller's own create timeout.
const CREATE_STREAM_IDLE_MARGIN: Duration = Duration::from_secs(5 * 60);

/// `CreateSpaceStream` through the daemon, forwarding its progress; a
/// daemon without it (`Unimplemented`) gets a plain `CreateSpace` and the
/// listener hears only `ready`.
async fn daemon_create_streaming(
    d: &DaemonClient,
    request: dpb::CreateSpaceRequest,
    listener: Arc<dyn SpaceCreateListener>,
) -> Result<dpb::CreateSpaceResponse> {
    let timeout = request
        .timeout
        .as_ref()
        .map(|t| Duration::from_secs(t.seconds.max(0) as u64));
    use dpb::create_space_stream_response::Event;
    let ready = || SpaceCreateProgress {
        phase: "ready".into(),
        fraction: None,
        detail: String::new(),
        bytes_done: None,
        bytes_total: None,
        bytes_per_second: None,
        space: String::new(),
    };
    let mut stream = match d
        .spaces()
        .create_space_stream(dpb::CreateSpaceStreamRequest {
            create: Some(request.clone()),
        })
        .await
    {
        Ok(s) => s.into_inner(),
        Err(e) if e.code() == tonic::Code::Unimplemented => {
            let r = d
                .spaces()
                .create_space(request)
                .await
                .map_err(daemon_err)?
                .into_inner();
            listener.on_progress(ready());
            return Ok(r);
        }
        Err(e) => return Err(daemon_err(e)),
    };
    // The daemon ends the stream after one result (or an error status).
    // Bounded: every phase of a create reports within its own budget (the
    // longest, the readiness wait, is `timeout`, 10 min by default), so a
    // stream silent for longer is a daemon that lost the create.
    let idle = timeout
        .map(|t| t + CREATE_STREAM_IDLE_MARGIN)
        .unwrap_or(CREATE_STREAM_IDLE)
        .max(CREATE_STREAM_IDLE);
    while let Some(msg) = tokio::time::timeout(idle, stream.message())
        .await
        .map_err(|_| {
            CuaError::Timeout(format!(
                "the cua daemon reported nothing about the create for {} min; see `cua daemon \
                 status` and `cua sb ls`",
                idle.as_secs() / 60
            ))
        })?
        .map_err(daemon_err)?
    {
        match msg.event {
            Some(Event::Progress(p)) => listener.on_progress(SpaceCreateProgress {
                phase: p.phase,
                fraction: p.fraction,
                detail: p.detail,
                bytes_done: p.bytes_done,
                bytes_total: p.bytes_total,
                bytes_per_second: p.bytes_per_second,
                space: p.space,
            }),
            Some(Event::Result(r)) => return Ok(r),
            None => {}
        }
    }
    Err(CuaError::Internal(
        "the daemon ended the create stream without a result".into(),
    ))
}

#[uniffi::export]
impl Spaces {
    /// Cancels a create that is still running: `space` is the create's
    /// `create_id`, the id the Space will have (`local:<name>`, carried by
    /// every progress report) or its name. The work in flight stops (an
    /// image download, a boot, a claim, a relay registration) and what the
    /// create made is removed (its VM or container and disks, its cloud
    /// claim, its relay machine); nothing that existed before is touched.
    /// Finished image downloads stay cached, so the next create resumes.
    /// Returns once the clean-up is done; the create itself fails with
    /// `Cancelled`. Idempotent: a second call says `not_creating`. Works
    /// for a create another process runs, and one a daemon restart cut off.
    pub async fn cancel_create(&self, space: String) -> Result<SpaceCancelOutcome> {
        let host = self.host.clone();
        run(async move {
            Ok(match &host {
                Host::Embedded(s) => {
                    let o = s.cancel_create(&space).await?;
                    SpaceCancelOutcome {
                        id: o.id,
                        state: o.state.as_str().into(),
                        message: o.message,
                    }
                }
                Host::Daemon(d) => {
                    let r = d
                        .spaces()
                        .cancel_create_space(dpb::CancelCreateSpaceRequest { space })
                        .await
                        .map_err(daemon_err)?
                        .into_inner();
                    SpaceCancelOutcome {
                        id: r.id,
                        state: r.state,
                        message: r.message,
                    }
                }
            })
        })
        .await
    }

    /// The GPU options each runtime of `on` (`local` by default, or
    /// `cloud`) offers on this host: Lume's "GPU acceleration" for macOS
    /// VMs (experimental, Apple silicon), QEMU's virgl and containers'
    /// NVIDIA GPUs on Linux. A runtime with no option says why.
    pub async fn gpu_support(&self, on: Option<String>) -> Result<Vec<GpuSupport>> {
        let host = self.host.clone();
        run(async move {
            let location = on.unwrap_or_default();
            Ok(match &host {
                Host::Embedded(s) => {
                    let on = if location.trim().is_empty() {
                        cua_sandbox_core::placement::On::Local
                    } else {
                        cua_sandbox_core::placement::On::parse(&location)
                            .map_err(|e| CuaError::InvalidPlacement(e.message))?
                    };
                    s.gpu_support(&on)
                        .await
                        .into_iter()
                        .map(Into::into)
                        .collect()
                }
                Host::Daemon(d) => d
                    .spaces()
                    .get_gpu_support(dpb::GetGpuSupportRequest { location })
                    .await
                    .map_err(daemon_err)?
                    .into_inner()
                    .runtimes
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            })
        })
        .await
    }

    /// Your machines that provide Spaces (the account's relay hosts, then
    /// the hosts added by their Tailscale or LAN address), each asked for
    /// its limits: what `create` takes as `on="host:<id>"`. One that does
    /// not answer is listed offline when it provided a Space before.
    pub async fn hosts(&self) -> Result<Vec<SpacesHost>> {
        let host = self.host.clone();
        run(async move {
            Ok(match &host {
                Host::Embedded(s) => s.hosts().await?.into_iter().map(Into::into).collect(),
                Host::Daemon(d) => d
                    .spaces()
                    .list_hosts(dpb::ListHostsRequest {})
                    .await
                    .map_err(daemon_err)?
                    .into_inner()
                    .hosts
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            })
        })
        .await
    }

    /// Deletes a Space's sandbox and forgets it (a Space added by address
    /// is only forgotten). Returns what happened.
    pub async fn delete(&self, space: String) -> Result<String> {
        let host = self.host.clone();
        run(async move {
            match &host {
                Host::Embedded(s) => Ok(s.delete(&space).await?),
                Host::Daemon(d) => Ok(d
                    .spaces()
                    .delete_space(dpb::DeleteSpaceRequest { space })
                    .await
                    .map_err(daemon_err)?
                    .into_inner()
                    .message),
            }
        })
        .await
    }

    /// Turns a Space off the way its provider can (`SpaceInfo.power`):
    /// suspends it, keeping its memory (a local container or QEMU VM), or
    /// stops it, keeping its disk (a local Lume VM; a Space one of your
    /// machines provides, which that machine stops). A cloud Space, a Space
    /// added by address and your own computers cannot be turned off.
    pub async fn stop(&self, space: String) -> Result<SpacePowerReport> {
        let host = self.host.clone();
        run(async move {
            parse(
                host.tool_value("stop_space", json!({"space": space}))
                    .await?,
            )
        })
        .await
    }

    /// Turns a Space on again: resumes a suspended one, boots a stopped one
    /// (a Space one of your machines provides joins the relay again), and
    /// leaves a running one as it is. Returns once it answers (bounded).
    pub async fn start(&self, space: String) -> Result<SpacePowerReport> {
        let host = self.host.clone();
        run(async move {
            parse(
                host.tool_value("start_space", json!({"space": space}))
                    .await?,
            )
        })
        .await
    }

    /// Publishes `space` on the cua.ai relay as a machine of the signed-in
    /// account, the way `cua host setup` publishes this computer: the
    /// Space's own driver dials out, so the account's other devices (a
    /// phone off this network) reach it as `relay:<machine>`, and nobody
    /// else until it is shared. Idempotent.
    pub async fn relay_register(&self, space: String) -> Result<SpaceRelayRegistration> {
        let host = self.host.clone();
        run(async move {
            parse(
                host.tool_value("relay_register_space", json!({"space": space}))
                    .await?,
            )
        })
        .await
    }

    /// Takes `space` off the relay (its driver leaves; every share goes).
    /// Returns false when it was not on the relay.
    pub async fn relay_unregister(&self, space: String) -> Result<bool> {
        let host = self.host.clone();
        run(async move {
            let v = host
                .tool_value("relay_unregister_space", json!({"space": space}))
                .await?;
            Ok(v.get("unregistered")
                .and_then(Value::as_bool)
                .unwrap_or(false))
        })
        .await
    }

    /// A connected handle to a registered Space.
    pub async fn space(&self, space: String) -> Result<Arc<Space>> {
        let host = self.host.clone();
        run(async move {
            let (info, inner) = host.connect(&space).await?;
            Ok(Arc::new(Space { host, info, inner }))
        })
        .await
    }

    /// What every agent harness is and cannot do (`agent_capabilities`),
    /// as JSON.
    pub async fn agent_capabilities(&self) -> Result<String> {
        let host = self.host.clone();
        run(async move {
            Ok(host
                .tool_value("agent_capabilities", json!({}))
                .await?
                .to_string())
        })
        .await
    }

    /// The Spaces MCP `tools/list` result as JSON (the contract).
    pub async fn list_tools_json(&self) -> Result<String> {
        let host = self.host.clone();
        run(async move {
            match &host {
                Host::Embedded(s) => Ok(cua_spaces::mcp::McpServer::new(s.clone())
                    .tools_list()
                    .to_string()),
                Host::Daemon(d) => Ok(d
                    .spaces()
                    .list_space_tools(dpb::ListSpaceToolsRequest {})
                    .await
                    .map_err(daemon_err)?
                    .into_inner()
                    .tools_json),
            }
        })
        .await
    }

    /// Calls any Spaces MCP tool with JSON arguments (the implementation
    /// `cua daemon mcp` serves). Tool errors are returned, not raised.
    pub async fn call_tool_json(
        &self,
        tool: String,
        arguments_json: Option<String>,
    ) -> Result<SpaceToolResult> {
        let host = self.host.clone();
        run(async move {
            if cua_spaces::contract::tool(&tool).is_none() {
                return Err(CuaError::NotFound(format!("Spaces tool {tool}")));
            }
            let args: Value = match arguments_json.as_deref().map(str::trim) {
                None | Some("") => json!({}),
                Some(s) => serde_json::from_str(s)?,
            };
            let o = host.tool(&tool, args).await?;
            Ok(SpaceToolResult::with_meta(
                o.content,
                o.structured,
                o.is_error,
                o.meta,
            ))
        })
        .await
    }
}

// ------------------------------------------------------------ Space

/// A connected Space. Every primitive checks the spacesd feature it
/// needs first and fails with `CapabilityMissing` naming it.
#[derive(uniffi::Object)]
pub struct Space {
    host: Host,
    info: SpaceInfo,
    inner: cua_spaces::Space,
}

impl Space {
    /// Rust hosts: the `cua_spaces::Space` handle.
    pub fn handle(&self) -> &cua_spaces::Space {
        &self.inner
    }
}

fn stream_target(
    s: &cua_spaces::Space,
    o: &SpaceStreamOptions,
) -> impl std::future::Future<Output = Result<cua_spaces::stream::StreamTarget>> + Send {
    let s = s.clone();
    let o = o.clone();
    async move {
        use cua_spaces::stream::StreamTarget;
        Ok(match (o.window_id, o.app_name) {
            (Some(w), _) if !w.is_empty() => StreamTarget::Window(w),
            (_, Some(app)) if !app.is_empty() => {
                StreamTarget::Window(s.find_window(&app).await?.window_id)
            }
            _ => StreamTarget::Display(o.display),
        })
    }
}

fn stream_options(o: &SpaceStreamOptions) -> Result<cua_spaces::stream::StreamOptions> {
    use cua_spacesd_client::pb::MediaCodec;
    let codecs = o
        .codecs
        .iter()
        .map(|c| match c.to_ascii_lowercase().as_str() {
            "h264" => Ok(MediaCodec::H264),
            "bgra" => Ok(MediaCodec::Bgra),
            "png" => Ok(MediaCodec::Png),
            other => Err(CuaError::InvalidArgument(format!(
                "unknown codec {other:?}"
            ))),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(cua_spaces::stream::StreamOptions {
        codecs,
        max_fps: o.max_fps,
        max_dimension: o.max_dimension,
        audio: o.audio,
        ticket_ttl: o.ticket_ttl_ms.map(Duration::from_millis),
        policy: match o.policy.as_deref() {
            None | Some("") => None,
            Some("view_only") => Some(cua_spacesd_client::pb::SessionPolicy::ViewOnly),
            Some("background_only") => Some(cua_spacesd_client::pb::SessionPolicy::BackgroundOnly),
            Some("allow_activation") => {
                Some(cua_spacesd_client::pb::SessionPolicy::AllowActivation)
            }
            Some(other) => {
                return Err(CuaError::InvalidArgument(format!(
                    "policy must be view_only, background_only or allow_activation (got {other:?})"
                )));
            }
        },
    })
}

#[uniffi::export]
impl Space {
    /// Id.
    pub fn id(&self) -> String {
        self.info.id.clone()
    }

    /// The registry entry.
    pub fn info(&self) -> SpaceInfo {
        self.info.clone()
    }

    /// Whether the spacesd supports `feature`.
    pub fn supports(&self, feature: String) -> bool {
        self.inner.supports(&feature)
    }

    /// `GetCapabilities` of the spacesd, as proto3 JSON.
    pub fn capabilities_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self.inner.capabilities())?)
    }

    /// Headers a media WebSocket to this Space needs besides its ticket.
    pub async fn websocket_headers(&self) -> Result<Vec<super::HttpHeader>> {
        let s = self.inner.clone();
        run(async move {
            Ok(s.websocket_headers()
                .await?
                .into_iter()
                .map(|(name, value)| super::HttpHeader { name, value })
                .collect())
        })
        .await
    }

    // ---- exec

    /// Runs a shell command (`/bin/sh -c`, `cmd /C` on Windows).
    pub async fn bash(&self, command: String, timeout_ms: Option<u64>) -> Result<SpaceBashResult> {
        let s = self.inner.clone();
        run(async move {
            let t = Duration::from_millis(timeout_ms.unwrap_or(60_000).clamp(1, 86_400_000));
            let o = s.bash(&command, t).await?;
            Ok(SpaceBashResult {
                rendered: o.render(),
                stdout: o.stdout,
                stderr: o.stderr,
                exit_code: o.exit_code,
                signal: o.signal,
                timed_out: o.timed_out,
                error: o.error,
            })
        })
        .await
    }

    /// Writes bytes to a guest path (parents created, verified by SHA-256).
    pub async fn write(&self, path: String, content: Vec<u8>) -> Result<SpaceWriteReport> {
        let s = self.inner.clone();
        run(async move { via_json(&s.write(&path, content).await?) }).await
    }

    /// The Space user's home directory.
    pub async fn home(&self) -> Result<String> {
        let s = self.inner.clone();
        run(async move { Ok(s.home().await?) }).await
    }

    // ---- files

    /// Copies a host file or folder to `dest` in the Space (default: the
    /// home directory).
    pub async fn upload(
        &self,
        local_path: String,
        dest: Option<String>,
    ) -> Result<SpaceTransferReport> {
        let s = self.inner.clone();
        run(async move {
            via_json(
                &s.upload(std::path::Path::new(&local_path), dest.as_deref())
                    .await?,
            )
        })
        .await
    }

    /// Copies a guest file or folder into `dest_dir` on this host. Existing
    /// host files are never replaced: a name that is taken lands as
    /// `name (1).ext`, and the report's `dest` is the path used. Use
    /// `download_file` on the Space's environment to write one exact path.
    pub async fn download(
        &self,
        remote_path: String,
        dest_dir: String,
    ) -> Result<SpaceTransferReport> {
        let s = self.inner.clone();
        run(async move {
            via_json(
                &s.download(&remote_path, std::path::Path::new(&dest_dir))
                    .await?,
            )
        })
        .await
    }

    /// Drops a host file or folder into the Space's `~/Downloads[/subdir]`
    /// (TeleportService.ReceiveFiles, per-file SHA-256).
    pub async fn send_file(
        &self,
        local_path: String,
        options: SpaceSendFileOptions,
    ) -> Result<SpaceSendFileReport> {
        let s = self.inner.clone();
        run(async move {
            use cua_spaces::files::{SendFileOptions, downloads_subdir};
            let needs_home = options
                .target_directory
                .as_deref()
                .is_some_and(|t| t.starts_with('/'));
            let home = if needs_home {
                s.home().await.ok()
            } else {
                None
            };
            let subdir = downloads_subdir(options.target_directory.as_deref(), home.as_deref())?;
            let conflict = parse_conflict(options.conflict.as_deref())?;
            via_json(
                &s.send_file(
                    std::path::Path::new(&local_path),
                    SendFileOptions {
                        subdir,
                        respect_ignore_files: options.respect_ignore_files,
                        conflict,
                    },
                )
                .await?,
            )
        })
        .await
    }

    // ---- services

    /// The MCP services this Space exposes: `driver` when its spacesd has
    /// the tool registry, then every declared service (reached over generic
    /// MCP; no spacesd needed).
    pub fn services(&self) -> Vec<String> {
        self.inner.services()
    }

    /// Tools of the Space's MCP service (`driver`, the default).
    pub async fn list_tools(&self, service: Option<String>) -> Result<Vec<SpaceToolInfo>> {
        let s = self.inner.clone();
        run(async move {
            let (tools, _) = s.list_tools(service.as_deref()).await?;
            Ok(tools
                .into_iter()
                .map(|t| SpaceToolInfo {
                    name: t.name,
                    description: t.description,
                    input_schema_json: t.input_schema.to_string(),
                    read_only: t.read_only,
                    destructive: t.destructive,
                })
                .collect())
        })
        .await
    }

    /// Captures the Space's display: PNG at full size on the primary
    /// display unless `options` say otherwise (for a small preview,
    /// [`Space::thumbnail`] reads the shared cache instead). Fails with
    /// `CapabilityMissing` (`spacesd`) when the image runs no cua-spacesd.
    pub async fn screenshot(
        &self,
        options: Option<crate::types::ScreenshotOptions>,
    ) -> Result<crate::types::Screenshot> {
        let s = self.inner.clone();
        let request = super::spacesd::screenshot_request(options);
        run(async move {
            let shot = s.spacesd()?.screenshot(request).await?;
            Ok(super::spacesd::screenshot_reply(shot))
        })
        .await
    }

    /// The Space's latest thumbnail (a small JPEG of its primary display)
    /// from the cache every client on this machine shares: returned at
    /// once when it is younger than `max_age_ms` (unset: any age), else
    /// captured fresh through cua-spacesd and kept for the next caller.
    /// When that capture fails, the older one comes back (`captured_at_ms`
    /// says how old). Asking keeps the daemon refreshing running Spaces'
    /// thumbnails in the background for a while (about every 90 s).
    pub async fn thumbnail(&self, max_age_ms: Option<u64>) -> Result<crate::types::SpaceThumbnail> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            let max_age = max_age_ms.map(Duration::from_millis);
            Ok(match &host {
                Host::Embedded(s) => {
                    let t = s.thumbnail(&id, max_age).await?;
                    crate::types::SpaceThumbnail {
                        format: thumbnail_format(&t.format),
                        width: t.width,
                        height: t.height,
                        captured_at_ms: t
                            .captured_at
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0),
                        image: t.image,
                    }
                }
                Host::Daemon(d) => {
                    let r = d
                        .spaces()
                        .get_space_thumbnail(dpb::GetSpaceThumbnailRequest {
                            space: id,
                            max_age: max_age.map(|d| pbjson_types::Duration {
                                seconds: d.as_secs() as i64,
                                nanos: d.subsec_nanos() as i32,
                            }),
                        })
                        .await
                        .map_err(daemon_err)?
                        .into_inner();
                    let captured_at_ms = r
                        .captured_at
                        .map(|t| {
                            (t.seconds.max(0) as u64) * 1000 + (t.nanos.max(0) as u64) / 1_000_000
                        })
                        .unwrap_or(0);
                    crate::types::SpaceThumbnail {
                        image: r.image,
                        format: thumbnail_format(&r.format),
                        width: r.width,
                        height: r.height,
                        captured_at_ms,
                    }
                }
            })
        })
        .await
    }

    /// Calls a tool of the Space's MCP service with JSON arguments.
    pub async fn call_tool(
        &self,
        tool: String,
        arguments_json: Option<String>,
        service: Option<String>,
        timeout_ms: Option<u64>,
    ) -> Result<SpaceToolResult> {
        let s = self.inner.clone();
        run(async move {
            let args = match arguments_json.as_deref().map(str::trim) {
                None | Some("") => serde_json::Map::new(),
                Some(j) => match serde_json::from_str::<Value>(j)? {
                    Value::Object(m) => m,
                    _ => {
                        return Err(CuaError::InvalidArgument(
                            "arguments_json must be a JSON object".into(),
                        ));
                    }
                },
            };
            let r = s
                .call_tool(
                    service.as_deref(),
                    &tool,
                    args,
                    timeout_ms.map(Duration::from_millis),
                )
                .await?;
            Ok(SpaceToolResult::with_meta(
                r.content,
                r.structured,
                r.is_error,
                r.meta,
            ))
        })
        .await
    }

    // ---- streams

    /// Streamable windows, optionally of one app.
    pub async fn windows(&self, app: Option<String>) -> Result<Vec<SpaceWindow>> {
        let s = self.inner.clone();
        run(async move { via_json(&s.windows(app.as_deref()).await?) }).await
    }

    /// A small JPEG preview of one of the Space's windows (`window_id` and
    /// `epoch` from `windows`), at most `max_dimension` px, reused for a
    /// few seconds (the SDK's preview cache); `None` when the guest
    /// captured nothing.
    pub async fn window_thumbnail(
        &self,
        window_id: String,
        epoch: u64,
        max_dimension: u32,
    ) -> Result<Option<Vec<u8>>> {
        let s = self.inner.clone();
        run(async move { Ok(s.window_thumbnail(&window_id, epoch, max_dimension).await?) }).await
    }

    /// Memory and storage use now (cheap: poll it at a low rate while a
    /// detail is visible).
    pub async fn usage(&self) -> Result<SpaceUsage> {
        let s = self.inner.clone();
        run(async move { via_json(&s.usage().await?) }).await
    }

    /// The Space's displays, primary first, with their resolution in
    /// physical pixels.
    pub async fn displays(&self) -> Result<Vec<SpaceDisplay>> {
        let s = self.inner.clone();
        run(async move { via_json(&s.displays().await?) }).await
    }

    /// The icon the guest desktop shows for a window's app (pass the
    /// window's `app_name`, `app_id` and `pid`), or `None` when the Space
    /// has none: show no icon then, not a placeholder. A window list should
    /// ask [`Space::app_icons`] once for all its windows instead.
    pub async fn app_icon(
        &self,
        app_name: String,
        app_id: String,
        pid: u32,
    ) -> Result<Option<SpaceAppIcon>> {
        Ok(self
            .app_icons(vec![SpaceAppIconRequest {
                app_name,
                app_id,
                pid,
            }])
            .await?
            .pop()
            .flatten())
    }

    /// Icons for many windows' apps at once, in request order (`None`: the
    /// Space has no icon for that app; show none). Answered by the SDK's
    /// one icon cache (memory, then `$CUA_HOME/cache/icons`, keyed by the
    /// app and the Space's image, never the pid); every miss costs one
    /// guest round trip for the whole batch. macOS renders the app bundle
    /// of the pid; Linux reads the `.desktop` entry's `Icon=` from the icon
    /// theme; Windows has none.
    pub async fn app_icons(
        &self,
        requests: Vec<SpaceAppIconRequest>,
    ) -> Result<Vec<Option<SpaceAppIcon>>> {
        let s = self.inner.clone();
        run(async move {
            let requests: Vec<cua_spaces::IconRequest> = requests
                .into_iter()
                .map(|r| cua_spaces::IconRequest {
                    app_name: r.app_name,
                    app_id: r.app_id,
                    pid: r.pid,
                })
                .collect();
            Ok(s.app_icons(&requests)
                .await?
                .into_iter()
                .map(|i| i.map(SpaceAppIcon::from))
                .collect())
        })
        .await
    }

    /// Mints a media session (ticket + WebSocket URL) for another client
    /// to attach to.
    pub async fn open_stream(&self, options: SpaceStreamOptions) -> Result<SpaceStreamTicket> {
        let s = self.inner.clone();
        let daemon = matches!(self.host, Host::Daemon(_));
        run(async move {
            let target = stream_target(&s, &options).await?;
            let t = s.open_stream(target, stream_options(&options)?).await?;
            Ok(SpaceStreamTicket {
                space: t.space,
                media_session_id: t.media_session_id,
                ws_url: t.ws_url,
                ticket: t.ticket,
                ticket_expires_at: t.ticket_expires_at,
                codec: t.codec,
                wire_version: t.wire_version,
                width: t.frame_size[0],
                height: t.frame_size[1],
                needs_headers: t.needs_gateway_headers || daemon,
            })
        })
        .await
    }

    /// Closes a media session opened with [`Space::open_stream`].
    pub async fn close_stream(&self, media_session_id: String) -> Result<()> {
        let s = self.inner.clone();
        run(async move { Ok(s.close_stream(&media_session_id).await?) }).await
    }

    /// Attaches to a ticket from [`Self::open_stream`] and delivers the
    /// media socket's encoded frames and audio packets to the sinks as they
    /// arrive. Keyframe gating, loss recovery and decoding are the
    /// caller's; [`Self::stream_session`] does them and ships with Cua
    /// Spaces. End the session with [`Self::close_stream`] and the ticket's
    /// `media_session_id`.
    pub async fn attach_stream(
        &self,
        ticket: SpaceStreamTicket,
        frames: Arc<dyn FrameSink>,
        audio: Option<Arc<dyn AudioSink>>,
    ) -> Result<Arc<super::MediaSession>> {
        let s = self.inner.clone();
        let headers = run(async move { Ok(s.websocket_headers().await?) }).await?;
        super::MediaSession::attach_url(ticket.ws_url, headers, frames, audio).await
    }

    /// Opens a media session and delivers keyframe-gated encoded frames
    /// and audio to the sinks (one delivery thread; decoding stays with the
    /// caller). The streaming client ships with Cua Spaces
    /// (source-available, FSL-1.1-MIT): without it this raises
    /// `HostCapabilityMissing`; [`Self::attach_stream`] needs no client.
    pub async fn stream_session(
        &self,
        options: SpaceStreamOptions,
        frames: Arc<dyn FrameSink>,
        audio: Option<Arc<dyn AudioSink>>,
    ) -> Result<Arc<SpaceStreamSession>> {
        let s = self.inner.clone();
        run(async move {
            let target = stream_target(&s, &options).await?;
            let frames: Arc<dyn cua_spaces::stream::FrameSink> = Arc::new(FrameAdapter(frames));
            let audio =
                audio.map(|a| Arc::new(AudioAdapter(a)) as Arc<dyn cua_spaces::stream::AudioSink>);
            let session = s
                .stream_session(target, stream_options(&options)?, frames, audio)
                .await?;
            Ok(Arc::new(SpaceStreamSession {
                media_session_id: session.media_session_id().to_string(),
                codec: session.codec().to_string(),
                inner: tokio::sync::Mutex::new(Some(session)),
            }))
        })
        .await
    }

    // ---- presence

    /// Joins presence. Waits up to `timeout_ms` (default 10 s) for the
    /// roster.
    pub async fn join_presence(
        &self,
        identity: PresenceIdentity,
        timeout_ms: Option<u64>,
    ) -> Result<Arc<SpacePresence>> {
        let s = self.inner.clone();
        let started = std::time::Instant::now();
        let r = run(async move {
            let session = s
                .join_presence(
                    cua_spaces::presence::Identity {
                        id: identity.id,
                        display_name: identity.display_name,
                        color: identity.color,
                        agent: identity.agent,
                    },
                    Duration::from_millis(timeout_ms.unwrap_or(10_000)),
                )
                .await?;
            let peak = session.roster().len() as u64;
            Ok(Arc::new(SpacePresence {
                sender: session.sender(),
                datagrams: session.uses_datagrams(),
                view_seed: session.view(),
                inner: Arc::new(tokio::sync::Mutex::new(Some(session))),
                stats: PresenceStats::new(peak),
            }))
        })
        .await;
        if r.is_err() {
            // A join that failed: counted as a session with nobody in it.
            cua_telemetry::capture(cua_telemetry::events::presence_session(
                cua_telemetry::Outcome::Error,
                0,
                started.elapsed(),
            ));
        }
        r
    }

    /// Sets this Space's presence settings. `cursor_probe`: whether
    /// cua-spacesd may read the real cursor shape by briefly moving the idle
    /// guest pointer to a participant's position (on by default); `None`
    /// leaves it unchanged.
    #[uniffi::method(default(cursor_probe = None))]
    pub async fn set_presence_settings(&self, cursor_probe: Option<bool>) -> Result<()> {
        let s = self.inner.clone();
        run(async move { Ok(s.set_presence_settings(cursor_probe).await?) }).await
    }

    // ---- teleport (host effects: run where the Spaces runtime lives)

    /// What teleporting `app` (`full` or `tabs`) would move from this host.
    pub async fn teleport_manifest(
        &self,
        app: String,
        scope: Option<String>,
    ) -> Result<TeleportManifest> {
        let host = self.host.clone();
        run(async move {
            parse(
                host.tool_value("teleport_manifest", json!({"app": app, "scope": scope}))
                    .await?,
            )
        })
        .await
    }

    /// Teleports `app`'s session into this Space. `approver` sees the
    /// manifest and returns the human's decision; `None` cancels (raises
    /// `TeleportRefused`). Sensitive items need an explicit acknowledgement.
    ///
    /// Through a daemon, this may need two calls (see [`TeleportReceipt`]):
    /// the first returns `status: "pending"` with a `request_id` once the
    /// user's approval is filed with the Cua Keyvault; call again with that
    /// same `request_id` (`approver` is not consulted again -- the decision
    /// it already made was what the first call filed) until `status:
    /// "moved"`. `request_id` is ignored for an embedded host, which always
    /// finishes in one call.
    #[uniffi::method(default(request_id = None))]
    pub async fn teleport(
        &self,
        app: String,
        scope: Option<String>,
        approver: Arc<dyn TeleportApprover>,
        request_id: Option<String>,
    ) -> Result<TeleportReceipt> {
        let started = std::time::Instant::now();
        let app_id = app.clone();
        let r = self.teleport_inner(app, scope, approver, request_id).await;
        super::telemetry::teleport(&app_id, "full", "app_with_state", started, &r, 0);
        r
    }
}

impl Space {
    async fn teleport_inner(
        &self,
        app: String,
        scope: Option<String>,
        approver: Arc<dyn TeleportApprover>,
        request_id: Option<String>,
    ) -> Result<TeleportReceipt> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        // A retry (the user already decided what to file; re-asking the
        // approver would ask it to decide a second time for nothing, since
        // the daemon ignores the selection on a retry call and only asks
        // the Keyvault whether the SAME request was approved).
        if let (Some(rid), Host::Daemon(_)) = (&request_id, &host) {
            let rid = rid.clone();
            return run(async move {
                let out = host
                    .tool(
                        "teleport_app",
                        json!({"space": id, "app": app, "request_id": rid}),
                    )
                    .await?;
                let v = out
                    .structured
                    .clone()
                    .or_else(|| serde_json::from_str(&out.text()).ok())
                    .unwrap_or(Value::Null);
                teleport_app_response(&v)
            })
            .await;
        }
        run(async move {
            let manifest: TeleportManifest = parse(
                host.tool_value("teleport_manifest", json!({"app": app, "scope": scope}))
                    .await?,
            )?;
            let m = manifest.clone();
            let decision = tokio::task::spawn_blocking(move || approver.approve(m))
                .await
                .map_err(|e| CuaError::Internal(format!("approver: {e}")))?
                .ok_or_else(|| CuaError::TeleportRefused("the approver declined".into()))?;
            match &host {
                // In process: this program exports with its own rights, and
                // the approver is its consent gate (as in the Spaces UI and
                // the interactive CLI).
                Host::Embedded(spaces) => teleport_embedded(spaces, &id, &manifest, decision).await,
                // Through the daemon: the `teleport_app` tool never delivers a
                // caller-approved session (the daemon would be a confused
                // deputy). It files a Keyvault request instead.
                Host::Daemon(_) => {
                    let out = host
                        .tool(
                            "teleport_app",
                            json!({
                                "space": id,
                                "app": manifest.app,
                                "scope": manifest.scope,
                                "include": decision.include,
                                "acknowledge_sensitive": decision.acknowledge_sensitive,
                            }),
                        )
                        .await?;
                    let v = out
                        .structured
                        .clone()
                        .or_else(|| serde_json::from_str(&out.text()).ok())
                        .unwrap_or(Value::Null);
                    teleport_app_response(&v)
                }
            }
        })
        .await
    }
}

#[uniffi::export]
impl Space {
    // ---- hotspot

    /// Starts the reverse-SOCKS hotspot: this host serves the Space's
    /// egress. `set_system_proxy` points the guest's proxy settings at it
    /// (default false here).
    pub async fn start_hotspot(
        &self,
        set_system_proxy: Option<bool>,
        bypass: Option<Vec<String>>,
    ) -> Result<SpaceHotspotStatus> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            parse(
                host.tool_value(
                    "hotspot_start",
                    json!({
                        "space": id,
                        "set_system_proxy": set_system_proxy.unwrap_or(false),
                        "bypass": bypass.unwrap_or_default(),
                    }),
                )
                .await?,
            )
        })
        .await
    }

    /// Stops this Space's hotspot. Returns the stopped Space ids.
    pub async fn stop_hotspot(&self) -> Result<Vec<String>> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            let v = host
                .tool_value("hotspot_stop", json!({"space": id}))
                .await?;
            parse(v.get("stopped").cloned().unwrap_or(json!([])))
        })
        .await
    }

    /// Signs in to a site in this Space's browser with a password the user
    /// saved in the Cua Keyvault. The first call files a request
    /// (`status == "pending"` with a `request_id`); the user approves it in
    /// Cua, then call again with that `request_id` and the Keyvault types
    /// the login through the Space's cua-driver (`status == "filled"`). A
    /// declined request is an error. The password never reaches the caller.
    pub async fn request_site_login(
        &self,
        url: String,
        options: Option<SiteLoginOptions>,
    ) -> Result<SiteLoginReport> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        let o = options.unwrap_or_default();
        run(async move {
            parse(
                host.tool_value(
                    "request_site_login",
                    json!({
                        "space": id,
                        "url": url,
                        "username": o.username,
                        "agent": o.agent,
                        "session": o.session,
                        "target_id": o.target_id,
                        "tab_id": o.tab_id,
                        "request_id": o.request_id,
                        "wait_secs": o.wait_secs,
                    }),
                )
                .await?,
            )
        })
        .await
    }

    /// This Space's hotspot status.
    pub async fn hotspot_status(&self) -> Result<Vec<SpaceHotspotStatus>> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            let v = host
                .tool_value("hotspot_status", json!({"space": id}))
                .await?;
            parse(v.get("hotspots").cloned().unwrap_or(json!([])))
        })
        .await
    }

    // ---- sharing

    /// Lets `who` (a verified email or an account id) watch (`viewer`,
    /// the default) or use (`editor`) this Space through the relay. A Space
    /// that is not a host is attached to the relay on its first share.
    pub async fn share(&self, who: String, role: Option<String>) -> Result<SpaceShares> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            parse(
                host.tool_value(
                    "share_space",
                    json!({"space": id, "who": who, "role": role.unwrap_or_else(|| "viewer".into())}),
                )
                .await?,
            )
        })
        .await
    }

    /// Stops sharing this Space with `who` at once, or with everyone when
    /// `who` is `None`.
    pub async fn unshare(&self, who: Option<String>) -> Result<SpaceShares> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            parse(
                host.tool_value("unshare_space", json!({"space": id, "who": who}))
                    .await?,
            )
        })
        .await
    }

    /// Who this Space is shared with, and who of them is connected now.
    pub async fn shares(&self) -> Result<SpaceShares> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            parse(
                host.tool_value("space_shares", json!({"space": id}))
                    .await?,
            )
        })
        .await
    }

    // ---- agents

    /// Starts an agent CLI (`claude-code`, `openai-codex`, ...) with a
    /// prompt, as a detached spacesd process.
    pub async fn agent_start(
        &self,
        agent: String,
        prompt: String,
        show: Option<bool>,
        options: Option<SpaceAgentOptions>,
    ) -> Result<AgentStartReport> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        let o = options.unwrap_or_default();
        run(async move {
            let v = host
                .tool_value(
                    "agent_start",
                    json!({"space": id, "agent": agent, "prompt": prompt, "show": show,
                           "env_from_host": o.env_from_host, "repo": o.repo, "branch": o.branch,
                           "cwd": o.cwd, "model": o.model, "base_url": o.base_url,
                           "exit_when_idle": o.exit_when_idle, "home": o.home, "env": o.env}),
                )
                .await?;
            let s = |k: &str| {
                v.get(k)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            Ok(AgentStartReport {
                run_id: s("run_id"),
                agent: s("agent"),
                space: s("space"),
                process_tag: s("process_tag"),
                notes: parse(v.get("notes").cloned().unwrap_or(json!([])))?,
                json: v.to_string(),
            })
        })
        .await
    }

    /// A run's status with the last `tail` lines of output.
    pub async fn agent_status(&self, run_id: String, tail: Option<u32>) -> Result<AgentRunStatus> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            AgentRunStatus::from_json(
                host.tool_value(
                    "agent_status",
                    json!({"space": id, "run_id": run_id, "tail": tail}),
                )
                .await?,
            )
        })
        .await
    }

    /// Sends a follow-up message to a run.
    pub async fn agent_message(
        &self,
        run_id: String,
        text: String,
        force: Option<bool>,
    ) -> Result<AgentActionReport> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            let v = host
                .tool_value(
                    "agent_message",
                    json!({"space": id, "run_id": run_id, "text": text, "force": force}),
                )
                .await?;
            Ok(AgentActionReport {
                run_id: v["run_id"].as_str().unwrap_or_default().into(),
                ok: v["delivered"].as_bool().unwrap_or(false),
                reason: v["reason"].as_str().unwrap_or_default().into(),
                json: v.to_string(),
            })
        })
        .await
    }

    /// Stops a run.
    pub async fn agent_stop(&self, run_id: String) -> Result<AgentActionReport> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            let v = host
                .tool_value("agent_stop", json!({"space": id, "run_id": run_id}))
                .await?;
            Ok(AgentActionReport {
                run_id: v["run_id"].as_str().unwrap_or_default().into(),
                ok: v["stopped"].as_bool().unwrap_or(false),
                reason: v["reason"].as_str().unwrap_or_default().into(),
                json: v.to_string(),
            })
        })
        .await
    }

    /// Normalized events after `cursor` (0: the start), as the
    /// `agent_events` JSON (`events`, `cursor`, `caught_up`, `status`).
    pub async fn agent_events(
        &self,
        run_id: String,
        cursor: Option<u64>,
        max: Option<u32>,
    ) -> Result<String> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            Ok(host
                .tool_value(
                    "agent_events",
                    json!({"space": id, "run_id": run_id, "cursor": cursor, "max": max}),
                )
                .await?
                .to_string())
        })
        .await
    }

    /// Cancels the run's turn in flight; the session stays open.
    pub async fn agent_interrupt(&self, run_id: String) -> Result<AgentActionReport> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            let v = host
                .tool_value("agent_interrupt", json!({"space": id, "run_id": run_id}))
                .await?;
            Ok(AgentActionReport {
                run_id: v["run_id"].as_str().unwrap_or_default().into(),
                ok: v["interrupted"].as_bool().unwrap_or(false),
                reason: v["status"].as_str().unwrap_or_default().into(),
                json: v.to_string(),
            })
        })
        .await
    }

    /// Every agent run in this Space.
    pub async fn agent_list(&self) -> Result<Vec<AgentRunStatus>> {
        let host = self.host.clone();
        let id = self.info.id.clone();
        run(async move {
            let v = host.tool_value("agent_list", json!({"space": id})).await?;
            v.get("runs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(AgentRunStatus::from_json)
                .collect()
        })
        .await
    }
}

// ------------------------------------------------------------ streams

struct FrameAdapter(Arc<dyn FrameSink>);

impl cua_spaces::stream::FrameSink for FrameAdapter {
    fn on_frame(&self, f: cua_spaces::stream::VideoFrame) {
        self.0.on_frame(super::VideoFrame {
            sequence: f.sequence,
            codec: f.codec,
            keyframe: f.keyframe,
            width: f.width,
            height: f.height,
            capture_timestamp_us: f.capture_timestamp_us,
            codec_epoch: f.codec_epoch,
            geometry_epoch: f.geometry_epoch,
            data: f.data,
            header_json: String::new(),
        });
    }

    fn on_event(&self, e: cua_spaces::stream::StreamEvent) {
        use cua_spaces::stream::StreamEvent as E;
        let (kind, json) = match e {
            E::Opened(v) => ("session_opened".to_string(), v.to_string()),
            E::AudioConfigured(v) => ("audio_config".to_string(), v.to_string()),
            E::Lifecycle(v) => ("lifecycle".to_string(), v.to_string()),
            E::Stats(v) => ("stats".to_string(), v.to_string()),
            E::Message(v) => (
                v.get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message")
                    .to_string(),
                v.to_string(),
            ),
            E::Closed { code, reason } => (
                "closed".to_string(),
                json!({"code": code, "reason": reason}).to_string(),
            ),
        };
        self.0.on_event(MediaEvent { kind, json });
    }
}

struct AudioAdapter(Arc<dyn AudioSink>);

impl cua_spaces::stream::AudioSink for AudioAdapter {
    fn on_audio(&self, p: cua_spaces::stream::AudioPacket) {
        self.0.on_audio(super::AudioPacket {
            track_id: p.track_id,
            sequence: p.sequence,
            pts_us: p.pts_us,
            frame_samples: p.frame_samples,
            config_epoch: 0,
            discontinuity: p.lost > 0,
            dtx: p.dtx,
            data: p.data,
        });
    }
}

/// A live Space media session (see [`Space::stream_session`]).
#[derive(uniffi::Object)]
pub struct SpaceStreamSession {
    media_session_id: String,
    codec: String,
    inner: tokio::sync::Mutex<Option<cua_spaces::stream::StreamSession>>,
}

#[uniffi::export]
impl SpaceStreamSession {
    /// Media session id.
    pub fn media_session_id(&self) -> String {
        self.media_session_id.clone()
    }

    /// Negotiated codec.
    pub fn codec(&self) -> String {
        self.codec.clone()
    }

    /// Delivery counters (zeros once closed).
    pub fn stats(&self) -> SpaceStreamStats {
        self.inner
            .try_lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.stats().into()))
            .unwrap_or(SpaceStreamStats {
                frames: 0,
                keyframes: 0,
                frames_dropped: 0,
                frames_gated: 0,
                keyframe_requests: 0,
                audio_packets: 0,
                audio_lost: 0,
                events: 0,
                malformed: 0,
            })
    }

    /// Whether the socket is still open.
    pub fn is_open(&self) -> bool {
        self.inner
            .try_lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.is_open()))
            .unwrap_or(false)
    }

    /// Asks the encoder for a keyframe.
    pub fn request_keyframe(&self) -> Result<()> {
        let g = self
            .inner
            .try_lock()
            .map_err(|_| CuaError::Internal("session busy".into()))?;
        let s = g
            .as_ref()
            .ok_or_else(|| CuaError::Closed("stream session".into()))?;
        Ok(s.request_keyframe()?)
    }

    /// Sends a raw JSON control message (for example input events).
    pub fn send_text(&self, json: String) -> Result<()> {
        let g = self
            .inner
            .try_lock()
            .map_err(|_| CuaError::Internal("session busy".into()))?;
        let s = g
            .as_ref()
            .ok_or_else(|| CuaError::Closed("stream session".into()))?;
        Ok(s.send_text(json)?)
    }

    /// Closes the socket and the media session. Returns the final counters.
    pub async fn close(&self) -> Result<SpaceStreamStats> {
        let s = self.inner.lock().await.take();
        run(async move {
            match s {
                Some(s) => Ok(s.close().await?.into()),
                None => Err(CuaError::Closed("stream session".into())),
            }
        })
        .await
    }
}

// ------------------------------------------------------------ presence

/// A joined presence session.
#[derive(uniffi::Object)]
pub struct SpacePresence {
    inner: Arc<tokio::sync::Mutex<Option<cua_spaces::presence::PresenceSession>>>,
    /// Publishes without the session lock, so `update_cursor` never waits
    /// for a pending `next_event`.
    sender: cua_spaces::presence::CursorSender,
    datagrams: bool,
    view_seed: cua_spaces::presence::PresenceView,
    stats: PresenceStats,
}

/// What `cua_presence_session` reports when the session ends: how long it
/// lasted and the most people seen at once (both bucketed when sent).
struct PresenceStats {
    joined: std::time::Instant,
    peak: std::sync::atomic::AtomicU64,
    recorded: std::sync::atomic::AtomicBool,
}

impl PresenceStats {
    fn new(peak: u64) -> Self {
        Self {
            joined: std::time::Instant::now(),
            peak: std::sync::atomic::AtomicU64::new(peak),
            recorded: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn saw(&self, present: usize) {
        self.peak
            .fetch_max(present as u64, std::sync::atomic::Ordering::Relaxed);
    }

    /// Records the session once (at leave, or when it is dropped).
    fn finish(&self) {
        if self
            .recorded
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        cua_telemetry::capture(cua_telemetry::events::presence_session(
            cua_telemetry::Outcome::Ok,
            self.peak.load(std::sync::atomic::Ordering::Relaxed),
            self.joined.elapsed(),
        ));
    }
}

impl Drop for SpacePresence {
    fn drop(&mut self) {
        self.stats.finish();
    }
}

impl SpacePresence {
    async fn with<T>(
        &self,
        f: impl FnOnce(&mut cua_spaces::presence::PresenceSession) -> T,
    ) -> Result<T> {
        let mut g = self.inner.lock().await;
        let s = g
            .as_mut()
            .ok_or_else(|| CuaError::Closed("presence session".into()))?;
        Ok(f(s))
    }
}

#[uniffi::export]
impl SpacePresence {
    /// This participant.
    pub async fn me(&self) -> Result<PresenceParticipant> {
        self.with(|s| s.me().clone().into()).await
    }

    /// Everyone present, with cursors when known.
    pub async fn roster(&self) -> Result<Vec<PresenceMember>> {
        self.with(|s| {
            self.stats.saw(s.roster().len());
            s.roster()
                .iter()
                .map(|(p, c)| PresenceMember {
                    participant: p.clone().into(),
                    cursor: c.clone().map(Into::into),
                })
                .collect()
        })
        .await
    }

    /// Moves this participant's cursor. Throttled to 30 Hz, newest wins:
    /// a move inside the interval is held and sent at its end, and show,
    /// hide and target changes go out at once. Never waits for
    /// `next_event`.
    pub async fn update_cursor(&self, cursor: PresenceCursor) -> Result<()> {
        let sender = self.sender.clone();
        run(async move { Ok(sender.update(&cursor.into()).await?) }).await
    }

    /// Whether cursors travel over the QUIC datagram channel (else the
    /// `Join` stream and `UpdateCursor`).
    pub fn uses_datagrams(&self) -> bool {
        self.datagrams
    }

    /// A fresh [`PresenceView`] seeded with the caller and the roster at
    /// join, with the render delay matching the transport. Feed it every
    /// event from `next_event`.
    pub fn view(&self) -> Arc<PresenceView> {
        PresenceView::wrap(self.view_seed.clone())
    }

    /// The next event, or `None` when `timeout_ms` (default 5 s) passes.
    pub async fn next_event(&self, timeout_ms: Option<u64>) -> Result<Option<PresenceEvent>> {
        let inner = self.inner.clone();
        run(async move {
            let mut g = inner.lock().await;
            let s = g
                .as_mut()
                .ok_or_else(|| CuaError::Closed("presence session".into()))?;
            let e = s
                .next_event(Duration::from_millis(timeout_ms.unwrap_or(5_000)))
                .await?;
            Ok((e, s.roster().len()))
        })
        .await
        .map(|(e, present)| {
            self.stats.saw(present);
            e.map(Into::into)
        })
    }

    /// Leaves.
    pub async fn leave(&self) -> Result<()> {
        self.stats.finish();
        let s = self.inner.lock().await.take();
        run(async move {
            match s {
                Some(s) => Ok(s.leave().await?),
                None => Ok(()),
            }
        })
        .await
    }
}

/// `SpaceSendFileOptions::conflict`: unset keeps both copies; replacing a
/// file in the Space takes an explicit `overwrite`.
fn parse_conflict(conflict: Option<&str>) -> Result<cua_spaces::files::Conflict> {
    use cua_spaces::files::Conflict;
    match conflict.unwrap_or("") {
        "rename" | "" => Ok(Conflict::Rename),
        "overwrite" => Ok(Conflict::Overwrite),
        "skip" => Ok(Conflict::Skip),
        other => Err(CuaError::InvalidArgument(format!(
            "conflict must be rename, skip or overwrite (got {other:?})"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn send_file_keeps_both_copies_unless_overwrite_is_explicit() {
        use cua_spaces::files::Conflict;
        assert_eq!(parse_conflict(None).unwrap(), Conflict::Rename);
        assert_eq!(parse_conflict(Some("")).unwrap(), Conflict::Rename);
        assert_eq!(parse_conflict(Some("rename")).unwrap(), Conflict::Rename);
        assert_eq!(parse_conflict(Some("skip")).unwrap(), Conflict::Skip);
        assert_eq!(
            parse_conflict(Some("overwrite")).unwrap(),
            Conflict::Overwrite
        );
        assert!(parse_conflict(Some("replace")).is_err());
    }

    /// The SDK retry-id bug this fixes: a first `teleport_app` call that
    /// only filed the Keyvault request (`consent_required: true`, no
    /// `status` key) must come back as `TeleportReceipt { status: "pending",
    /// request_id: Some(_) }`, never an `Err` -- a caller that only checks
    /// `Result::Err` must not mistake "ask the user" for "refused".
    #[test]
    fn a_filed_request_is_pending_with_a_retryable_request_id_not_an_error() {
        let v = json!({
            "consent_required": true,
            "moved": false,
            "request_id": "req-1",
            "app": "chrome",
            "space": "dev-1",
        });
        let r = teleport_app_response(&v).unwrap();
        assert_eq!(r.status, "pending");
        assert_eq!(r.request_id.as_deref(), Some("req-1"));
    }

    /// A still-pending retry (`status: "pending"` explicitly, same shape the
    /// tool sends on a retry before the user has decided) is also pending,
    /// not an error, and keeps the SAME request id to retry with again.
    #[test]
    fn a_still_pending_retry_stays_pending_with_the_same_request_id() {
        let v = json!({
            "consent_required": true,
            "moved": false,
            "request_id": "req-1",
            "status": "pending",
        });
        let r = teleport_app_response(&v).unwrap();
        assert_eq!(r.status, "pending");
        assert_eq!(r.request_id.as_deref(), Some("req-1"));
    }

    /// Once the user approves and the broker delivers, `moved: true` is a
    /// completed teleport: `status: "moved"`, no `request_id` to retry with.
    #[test]
    fn a_granted_and_delivered_request_is_moved() {
        let v = json!({
            "consent_required": false,
            "moved": true,
            "app": "chrome",
            "space": "dev-1",
            "items": ["item-1"],
            "transferred_paths": ["tabs.json", "cookies.json"],
            "import_ids": ["cua-kv-1"],
            "expires_ms": 123,
        });
        let r = teleport_app_response(&v).unwrap();
        assert_eq!(r.status, "moved");
        assert_eq!(r.request_id, None);
        assert_eq!(r.app, "chrome");
        assert_eq!(r.space, "dev-1");
        assert_eq!(r.method, "import_session");
        assert_eq!(r.transferred_paths, ["tabs.json", "cookies.json"]);
        assert_eq!(r.imported, ["tabs.json", "cookies.json"]);
    }

    /// The user declining (no `request_id` to retry: the request is done)
    /// is a real refusal, not a pending state.
    #[test]
    fn a_denied_request_is_a_refusal_not_pending() {
        let v = json!({
            "moved": false,
            "denied": true,
            "message": "the user declined the teleport (user); do not ask again unless they bring it up",
        });
        let err = teleport_app_response(&v).unwrap_err();
        assert!(matches!(err, CuaError::TeleportRefused(_)), "{err:?}");
        match err {
            CuaError::TeleportRefused(m) => assert!(m.contains("declined")),
            _ => unreachable!(),
        }
    }

    /// `requires_cua_app`-shaped error responses (no daemon Keyvault
    /// reachable at all) also fall through to the refusal, with its
    /// message, not a bogus "pending".
    #[test]
    fn an_error_response_is_a_refusal_with_its_message() {
        let v = json!({
            "moved": false,
            "error": {"code": "requires_cua_app", "message": "install Cua"},
        });
        let err = teleport_app_response(&v).unwrap_err();
        match err {
            CuaError::TeleportRefused(m) => assert!(m.contains("install Cua")),
            other => panic!("{other:?}"),
        }
    }

    /// The conformance vectors through the exported surface: every event
    /// survives the record round trip and the exported view draws what the
    /// core draws.
    #[test]
    fn presence_view_runs_the_conformance_vectors_through_the_bindings() {
        let doc: Value = serde_json::from_str(&presence_conformance_json()).unwrap();
        for case in doc["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let view = PresenceView::new(
                case["me"].as_str().unwrap().into(),
                case["delay_ms"].as_f64(),
            );
            for step in case["steps"].as_array().unwrap() {
                let at = step["at"].as_f64().unwrap();
                if let Some(e) = step.get("event") {
                    let core: cua_spaces::presence::PresenceEvent =
                        serde_json::from_value(e.clone()).unwrap();
                    let record: PresenceEvent = core.clone().into();
                    assert_eq!(record.to_core(), Some(core), "{name}");
                    view.apply(record, at);
                    continue;
                }
                let pointer = step["pointer"].as_array().map(|p| PresencePoint {
                    x: p[0].as_f64().unwrap(),
                    y: p[1].as_f64().unwrap(),
                });
                let got = view.drawables(at, pointer);
                let want = step["expect"].as_array().unwrap();
                assert_eq!(got.len(), want.len(), "{name}: {got:?}");
                for (g, w) in got.iter().zip(want) {
                    assert_eq!(g.participant_id, w["participant_id"].as_str().unwrap());
                    assert!(
                        (g.x - w["x"].as_f64().unwrap()).abs() < 1e-6,
                        "{name} {g:?}"
                    );
                    assert!((g.alpha - w["alpha"].as_f64().unwrap()).abs() < 1e-6);
                    assert_eq!(g.shape, w["shape"].as_str().unwrap());
                }
            }
        }
    }

    #[test]
    fn presence_art_is_exported_for_every_shape() {
        let all = presence_cursor_art_all();
        assert_eq!(all.len(), 14);
        assert_eq!(presence_cursor_art("text".into()).shape, "text");
        assert_eq!(presence_cursor_art("nonsense".into()).shape, "arrow");
        assert!(
            presence_cursor_art_svg("pointer".into(), "#3cb44b".into(), 24.0).contains("#3cb44b")
        );
    }

    #[test]
    fn an_agent_list_row_keeps_its_published_accepts_message() {
        // The row shape `agent_list` serves: an idle run accepts a follow-up.
        let row = json!({"run_id": "run-1", "agent": "goose", "status": "idle",
            "phase": "waiting", "reason": "waiting for a follow-up", "turn": 1,
            "alive": true, "accepts_message": true});
        let s = AgentRunStatus::from_json(row).unwrap();
        assert_eq!(s.status, "idle");
        assert!(s.accepts_message);
        assert_eq!(s.reason, "waiting for a follow-up");
    }

    #[test]
    fn every_contract_tool_maps_to_an_sdk_method() {
        let contract: BTreeSet<&str> = cua_spaces::contract::tools()
            .iter()
            .map(|t| t.name)
            .collect();
        let mapped: BTreeSet<&str> = SPACES_TOOL_METHODS.iter().map(|(t, _)| *t).collect();
        assert_eq!(
            mapped, contract,
            "SPACES_TOOL_METHODS must list exactly the Spaces contract tools"
        );
    }

    #[test]
    fn rfc3339_matches_known_instants() {
        let at = std::time::UNIX_EPOCH + Duration::from_secs(1_758_300_000);
        assert_eq!(humantime_rfc3339(at), "2025-09-19T16:40:00Z");
    }
}

// ------------------------------------------------------------ Cua Volume

/// Whose view of the drive a call takes: the user (the default, the whole
/// drive) or a persistent agent in a Space. It only ever narrows.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct DriveView {
    /// A persistent agent's name (`ada`).
    #[uniffi(default = None)]
    pub as_agent: Option<String>,
    /// The Space id the agent is in (`local:work`).
    #[uniffi(default = None)]
    pub in_space: Option<String>,
}

/// One row of a drive listing.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveEntry {
    /// Key (folders end in `/`).
    pub path: String,
    pub name: String,
    pub folder: bool,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub modified_ms: u64,
    #[serde(default)]
    pub etag: String,
    /// `r` or `rw`: what this view may do there.
    pub mode: String,
    /// A file's sync state, when it has something to say (uploading,
    /// conflicted, or last written by another device).
    #[serde(default)]
    pub sync: Option<DriveFileSync>,
}

/// One file's sync state.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveFileSync {
    /// `synced`, `pending_upload` (not yet in storage: other devices do not
    /// see it), `conflict` (a write lost to a later one, kept at
    /// `conflict_path`) or `conflict_copy` (the kept copy of a losing write).
    pub state: String,
    /// The device that last wrote it, when another one did.
    #[serde(default)]
    pub written_by: Option<String>,
    #[serde(default)]
    pub conflict_path: Option<String>,
}

/// A file waiting to upload.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DrivePendingUpload {
    pub path: String,
    pub bytes: u64,
}

/// The volume mounted in a Space's guest.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveSpaceVolume {
    pub space: String,
    /// `/volume` (Linux) or `~/Cua Volume` (macOS), as the guest sees it.
    pub mount_path: String,
    /// `fs` (FUSE) or `nfs`.
    pub backend: String,
    /// Whose view it shows: `space:<folder>`, or `agent:<name>` while a
    /// persistent agent runs there.
    pub principal: String,
}

/// A folder's listing.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveListing {
    pub path: String,
    /// `user` or `agent:<name>`.
    pub principal: String,
    pub entries: Vec<DriveEntry>,
}

/// A file's content and version.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DriveFile {
    pub path: String,
    pub size: u64,
    pub etag: String,
    pub version: String,
    pub modified_ms: u64,
    pub content: Vec<u8>,
    /// Its sync state, while the drive's services run.
    pub sync: Option<DriveFileSync>,
}

/// A written version.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveObject {
    pub key: String,
    pub size: u64,
    pub etag: String,
    pub version: String,
    pub modified_ms: u64,
}

/// One version in a file's history.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveVersion {
    pub version: String,
    pub size: u64,
    pub modified_ms: u64,
    pub deleted: bool,
    pub latest: bool,
}

/// A widening of an agent's or a Space's access.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveGrant {
    pub id: String,
    pub principal: String,
    pub prefix: String,
    /// `r` or `rw`.
    pub mode: String,
    pub created_ms: u64,
    #[serde(default)]
    pub expires_ms: Option<u64>,
    #[serde(default)]
    pub revoked: bool,
    #[serde(default)]
    pub note: String,
}

/// An agent's request for more access, waiting for the user.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveAccessRequest {
    pub id: String,
    pub principal: String,
    pub prefix: String,
    pub mode: String,
    #[serde(default)]
    pub reason: String,
    pub created_ms: u64,
}

/// One audit event.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveAuditEvent {
    pub seq: u64,
    pub ts_ms: u64,
    pub principal: String,
    pub action: String,
    pub path: String,
    #[serde(default)]
    pub detail: String,
}

/// The newest audit events and whether the hash chain verified.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveAudit {
    pub events: Vec<DriveAuditEvent>,
    pub verified: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Where an S3-compatible bucket is (no keys).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize, Serialize)]
pub struct DriveS3Settings {
    /// `http://127.0.0.1:9000`, `https://<account>.r2.cloudflarestorage.com`;
    /// `None` for AWS.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// `us-east-1`; `auto` for R2.
    #[serde(default)]
    pub region: String,
    pub bucket: String,
    /// A key prefix inside the bucket (may be empty).
    #[serde(default)]
    pub root: String,
    /// Path-style addressing (MinIO and most self-hosted stores).
    #[serde(default)]
    pub path_style: bool,
}

/// Where the drive keeps its bytes (Settings > Storage).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveStorage {
    /// `fs` or `s3`.
    pub backend: String,
    /// Where the `fs` backend keeps bytes.
    pub fs_path: String,
    #[serde(default)]
    pub s3: Option<DriveS3Settings>,
    /// S3 keys are saved (they are never returned).
    pub has_keys: bool,
    /// Always false in this release: hide the Cua cloud option.
    pub cloud_available: bool,
}

/// A storage change, or with `dry_run` a connection test.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Serialize)]
pub struct DriveStorageUpdate {
    /// `fs` or `s3` (`cloud` is refused).
    pub backend: String,
    pub s3: Option<DriveS3Settings>,
    /// With `secret_access_key` (both or neither); saved in the credential
    /// store.
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    /// Only test; change nothing.
    pub dry_run: bool,
}

/// What a storage test found.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveStorageCheck {
    pub ok: bool,
    pub reachable: bool,
    pub authorized: bool,
    /// Bucket versioning is on (required).
    pub versioning: bool,
    /// A sentence when not ok: what is wrong and how to fix it.
    #[serde(default)]
    pub detail: Option<String>,
    /// What is wrong, to key the UI on: `unreachable`, `bad_keys`,
    /// `forbidden`, `bucket_missing`, `versioning_off`,
    /// `path_style_needed`.
    #[serde(default)]
    pub problem: Option<String>,
    /// Saved and switched live.
    pub applied: bool,
}

/// The drive as a volume.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveMountStatus {
    /// The user's opt-in (off by default).
    pub enabled: bool,
    /// `off`, `mounting`, `mounted`, `needs_approval`, `unsupported`, `error`.
    pub state: String,
    /// `nfs` (macOS), `fuse` (Linux), `fskit`, `none`.
    pub method: String,
    /// The mount point when mounted ("Show in Finder" opens it).
    #[serde(default)]
    pub path: Option<String>,
    pub volume_name: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub settings_url: Option<String>,
}

/// A write that lost to a later one, kept in history and as a copy.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveConflict {
    pub path: String,
    pub conflict_path: String,
    pub winner_device: String,
    pub loser_device: String,
    pub winner_version: String,
    pub loser_version: String,
    pub ts_ms: u64,
}

/// A device sharing the drive's bucket.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveDevice {
    pub id: String,
    pub name: String,
    pub this_device: bool,
    pub last_seen_ms: u64,
    pub last_change_ms: u64,
    pub changes: u64,
}

/// Sync across devices.
#[derive(Debug, Clone, PartialEq, uniffi::Record, Deserialize)]
pub struct DriveSyncStatus {
    pub device_id: String,
    pub device_name: String,
    /// `live`, `off` (a store on this machine), `offline` (the bucket
    /// stopped answering; see `last_error`).
    pub feed: String,
    pub poll_interval_ms: u64,
    pub last_poll_ms: u64,
    pub last_remote_change_ms: u64,
    pub pending_uploads: u32,
    pub pending_bytes: u64,
    pub conflicts: Vec<DriveConflict>,
    pub devices: Vec<DriveDevice>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// The files still uploading, largest first (at most 100).
    #[serde(default)]
    pub pending: Vec<DrivePendingUpload>,
    /// `fs` (This Mac) or `s3` (your bucket).
    #[serde(default)]
    pub backend: String,
    /// This machine's mount: `off`, `mounting`, `mounted`,
    /// `needs_approval`, `unsupported` or `error`.
    #[serde(default)]
    pub mount: String,
    /// The block cache in front of a bucket.
    #[serde(default)]
    pub cache: Option<DriveCacheStats>,
    /// The volume mounted in Spaces.
    #[serde(default)]
    pub volumes: Vec<DriveSpaceVolume>,
}

/// One sync event.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveSyncEvent {
    pub seq: u64,
    pub ts_ms: u64,
    /// `remote_change`, `remote_delete`, `upload_started`, `upload_done`,
    /// `upload_failed`, `conflict`, `error`.
    pub kind: String,
    pub path: String,
    pub device: String,
    pub size: u64,
    pub version: String,
    pub detail: String,
}

/// Events after a sequence number, and the sequence to ask after next.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Deserialize)]
pub struct DriveSyncEvents {
    pub events: Vec<DriveSyncEvent>,
    pub next_seq: u64,
}

/// The block cache in front of a remote store.
#[derive(Debug, Clone, PartialEq, uniffi::Record, Deserialize)]
pub struct DriveCacheStats {
    pub dir: String,
    pub size_bytes: u64,
    pub capacity_bytes: u64,
    pub block_bytes: u64,
    pub blocks: u64,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub prefetched_bytes: u64,
    pub evictions: u64,
}

fn with_view(mut args: Value, view: Option<DriveView>) -> Value {
    if let Some(v) = view {
        if let Some(a) = v.as_agent {
            args["as_agent"] = json!(a);
        }
        if let Some(s) = v.in_space {
            args["in_space"] = json!(s);
        }
    }
    args
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Cua Volume: the versioned volume every Space and agent shares
/// (`public/`, `agents/<agent>/`, `spaces/<space>/`). The same tools
/// `cua daemon mcp` serves; access is checked by the runtime.
#[uniffi::export]
impl Spaces {
    /// `volume_ls`: a folder's immediate children (default the root).
    pub async fn volume_ls(
        &self,
        path: Option<String>,
        view: Option<DriveView>,
    ) -> Result<DriveListing> {
        let host = self.host.clone();
        run(async move {
            let args = with_view(json!({"path": path.unwrap_or_default()}), view);
            parse(host.tool_value("volume_ls", args).await?)
        })
        .await
    }

    /// `volume_read`: a file (or one of its versions).
    pub async fn volume_read(
        &self,
        path: String,
        version: Option<String>,
        view: Option<DriveView>,
    ) -> Result<DriveFile> {
        use base64::Engine as _;
        let host = self.host.clone();
        run(async move {
            let mut args = json!({"path": path});
            if let Some(v) = version {
                args["version"] = json!(v);
            }
            let v = host
                .tool_value("volume_read", with_view(args, view))
                .await?;
            let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
            let content = match v["encoding"].as_str() {
                Some("base64") => b64()
                    .decode(s("content"))
                    .map_err(|e| CuaError::Internal(format!("volume_read content: {e}")))?,
                _ => s("content").into_bytes(),
            };
            Ok(DriveFile {
                path: s("path"),
                size: v["size"].as_u64().unwrap_or(0),
                etag: s("etag"),
                version: s("version"),
                modified_ms: v["modified_ms"].as_u64().unwrap_or(0),
                content,
                sync: serde_json::from_value(v["sync"].clone()).ok(),
            })
        })
        .await
    }

    /// `volume_write`: a new version of a file. `if_etag` makes it a
    /// compare-and-swap, `create_only` makes it create-only.
    pub async fn volume_write(
        &self,
        path: String,
        content: Vec<u8>,
        if_etag: Option<String>,
        create_only: bool,
        view: Option<DriveView>,
    ) -> Result<DriveObject> {
        use base64::Engine as _;
        let host = self.host.clone();
        run(async move {
            let mut args = json!({"path": path, "content": b64().encode(&content),
                "encoding": "base64", "create_only": create_only});
            if let Some(e) = if_etag {
                args["if_etag"] = json!(e);
            }
            parse(
                host.tool_value("volume_write", with_view(args, view))
                    .await?,
            )
        })
        .await
    }

    /// `volume_delete`: a delete marker (history stays).
    pub async fn volume_delete(
        &self,
        path: String,
        if_etag: Option<String>,
        view: Option<DriveView>,
    ) -> Result<()> {
        let host = self.host.clone();
        run(async move {
            let mut args = json!({"path": path});
            if let Some(e) = if_etag {
                args["if_etag"] = json!(e);
            }
            host.tool_value("volume_delete", with_view(args, view))
                .await?;
            Ok(())
        })
        .await
    }

    /// `volume_history`: a file's versions, newest first.
    pub async fn volume_history(
        &self,
        path: String,
        view: Option<DriveView>,
    ) -> Result<Vec<DriveVersion>> {
        let host = self.host.clone();
        run(async move {
            let v = host
                .tool_value("volume_history", with_view(json!({"path": path}), view))
                .await?;
            parse(v["versions"].clone())
        })
        .await
    }

    /// `volume_restore`: makes an old version current again.
    pub async fn volume_restore(
        &self,
        path: String,
        version: String,
        view: Option<DriveView>,
    ) -> Result<DriveObject> {
        let host = self.host.clone();
        run(async move {
            let args = with_view(json!({"path": path, "version": version}), view);
            parse(host.tool_value("volume_restore", args).await?)
        })
        .await
    }

    /// `volume_grant`: widens `principal`'s access (`agent:<name>` or
    /// `space:<id>`, mode `r` or `rw`). The user confirms with presence.
    pub async fn volume_grant(
        &self,
        principal: String,
        prefix: String,
        mode: String,
        expires_in_secs: Option<u64>,
        note: Option<String>,
    ) -> Result<DriveGrant> {
        let host = self.host.clone();
        run(async move {
            let args = json!({"principal": principal, "prefix": prefix, "mode": mode,
                "expires_in_secs": expires_in_secs, "note": note});
            parse(host.tool_value("volume_grant", args).await?)
        })
        .await
    }

    /// `volume_revoke`.
    pub async fn volume_revoke(&self, grant_id: String) -> Result<DriveGrant> {
        let host = self.host.clone();
        run(async move {
            parse(
                host.tool_value("volume_revoke", json!({"grant_id": grant_id}))
                    .await?,
            )
        })
        .await
    }

    /// `volume_grants`: live grants (every grant with `all`).
    pub async fn volume_grants(&self, all: bool) -> Result<Vec<DriveGrant>> {
        let host = self.host.clone();
        run(async move {
            let v = host
                .tool_value("volume_grants", json!({"all": all}))
                .await?;
            parse(v["grants"].clone())
        })
        .await
    }

    /// `volume_request_access`: `agent` asks the user for more access.
    pub async fn volume_request_access(
        &self,
        agent: String,
        prefix: String,
        mode: String,
        reason: Option<String>,
        in_space: Option<String>,
    ) -> Result<DriveAccessRequest> {
        let host = self.host.clone();
        run(async move {
            let args = json!({"as_agent": agent, "in_space": in_space, "prefix": prefix,
                "mode": mode, "reason": reason});
            parse(host.tool_value("volume_request_access", args).await?)
        })
        .await
    }

    /// `volume_requests`: requests waiting for the user.
    pub async fn volume_requests(&self) -> Result<Vec<DriveAccessRequest>> {
        let host = self.host.clone();
        run(async move {
            let v = host.tool_value("volume_requests", json!({})).await?;
            parse(v["requests"].clone())
        })
        .await
    }

    /// `volume_approve`: the request becomes a grant (with presence).
    pub async fn volume_approve(
        &self,
        request_id: String,
        expires_in_secs: Option<u64>,
    ) -> Result<DriveGrant> {
        let host = self.host.clone();
        run(async move {
            let args = json!({"request_id": request_id, "expires_in_secs": expires_in_secs});
            parse(host.tool_value("volume_approve", args).await?)
        })
        .await
    }

    /// `volume_deny`.
    pub async fn volume_deny(&self, request_id: String) -> Result<()> {
        let host = self.host.clone();
        run(async move {
            host.tool_value("volume_deny", json!({"request_id": request_id}))
                .await?;
            Ok(())
        })
        .await
    }

    /// `volume_storage`: where the drive keeps its bytes.
    pub async fn volume_storage(&self) -> Result<DriveStorage> {
        let host = self.host.clone();
        run(async move { parse(host.tool_value("volume_storage", json!({})).await?) }).await
    }

    /// `volume_storage_set`: tests, and unless `dry_run` saves and switches
    /// to, a storage backend (live, no restart).
    pub async fn volume_storage_set(
        &self,
        update: DriveStorageUpdate,
    ) -> Result<DriveStorageCheck> {
        let host = self.host.clone();
        run(async move {
            let args = serde_json::to_value(&update)
                .map_err(|e| CuaError::Internal(format!("volume_storage_set: {e}")))?;
            parse(host.tool_value("volume_storage_set", args).await?)
        })
        .await
    }

    /// `volume_mount_status`: whether the drive is mounted as a volume.
    pub async fn volume_mount_status(&self) -> Result<DriveMountStatus> {
        let host = self.host.clone();
        run(async move { parse(host.tool_value("volume_mount_status", json!({})).await?) }).await
    }

    /// `volume_mount`: turns the mount on (kept across restarts) and mounts.
    pub async fn volume_mount(&self) -> Result<DriveMountStatus> {
        let host = self.host.clone();
        run(async move { parse(host.tool_value("volume_mount", json!({})).await?) }).await
    }

    /// `volume_unmount`: turns the mount off; pending uploads land first.
    pub async fn volume_unmount(&self) -> Result<DriveMountStatus> {
        let host = self.host.clone();
        run(async move { parse(host.tool_value("volume_unmount", json!({})).await?) }).await
    }

    /// `volume_sync_status`: devices, pending uploads and conflicts.
    pub async fn volume_sync_status(&self) -> Result<DriveSyncStatus> {
        let host = self.host.clone();
        run(async move { parse(host.tool_value("volume_sync_status", json!({})).await?) }).await
    }

    /// `volume_sync_events`: events after `since_seq`, waiting up to
    /// `wait_ms` (at most 30000) for one.
    pub async fn volume_sync_events(
        &self,
        since_seq: Option<u64>,
        wait_ms: Option<u32>,
    ) -> Result<DriveSyncEvents> {
        let host = self.host.clone();
        run(async move {
            parse(
                host.tool_value(
                    "volume_sync_events",
                    json!({"since_seq": since_seq, "wait_ms": wait_ms}),
                )
                .await?,
            )
        })
        .await
    }

    /// `volume_sync_resolve`: clears a conflict from the list (files stay).
    pub async fn volume_sync_resolve(&self, path: String) -> Result<()> {
        let host = self.host.clone();
        run(async move {
            host.tool_value("volume_sync_resolve", json!({"path": path}))
                .await?;
            Ok(())
        })
        .await
    }

    /// `volume_cache_stats`: the block cache's size, cap and hit rate.
    pub async fn volume_cache_stats(&self) -> Result<DriveCacheStats> {
        let host = self.host.clone();
        run(async move { parse(host.tool_value("volume_cache_stats", json!({})).await?) }).await
    }

    /// `volume_cache_set`: the cache's size cap (at least 256 MiB).
    pub async fn volume_cache_set(&self, capacity_bytes: u64) -> Result<DriveCacheStats> {
        let host = self.host.clone();
        run(async move {
            parse(
                host.tool_value(
                    "volume_cache_set",
                    json!({"capacity_bytes": capacity_bytes}),
                )
                .await?,
            )
        })
        .await
    }

    /// `volume_cache_clear`: drops every cached block.
    pub async fn volume_cache_clear(&self) -> Result<DriveCacheStats> {
        let host = self.host.clone();
        run(async move { parse(host.tool_value("volume_cache_clear", json!({})).await?) }).await
    }

    /// `volume_audit`: the newest events (default 50) and whether the log
    /// verified.
    pub async fn volume_audit(&self, limit: Option<u32>) -> Result<DriveAudit> {
        let host = self.host.clone();
        run(async move {
            parse(
                host.tool_value("volume_audit", json!({"limit": limit}))
                    .await?,
            )
        })
        .await
    }
}

/// The drive method names from before the rename to Cua Volume, for Rust
/// callers only and for one release (the language bindings have the new
/// names only).
impl Spaces {
    #[deprecated(note = "renamed to `volume_ls` (Cua Volume); removed after this release")]
    pub async fn drive_ls(
        &self,
        path: Option<String>,
        view: Option<DriveView>,
    ) -> Result<DriveListing> {
        self.volume_ls(path, view).await
    }

    #[deprecated(note = "renamed to `volume_read` (Cua Volume); removed after this release")]
    pub async fn drive_read(
        &self,
        path: String,
        version: Option<String>,
        view: Option<DriveView>,
    ) -> Result<DriveFile> {
        self.volume_read(path, version, view).await
    }

    #[deprecated(note = "renamed to `volume_write` (Cua Volume); removed after this release")]
    pub async fn drive_write(
        &self,
        path: String,
        content: Vec<u8>,
        if_etag: Option<String>,
        create_only: bool,
        view: Option<DriveView>,
    ) -> Result<DriveObject> {
        self.volume_write(path, content, if_etag, create_only, view)
            .await
    }

    #[deprecated(note = "renamed to `volume_delete` (Cua Volume); removed after this release")]
    pub async fn drive_delete(
        &self,
        path: String,
        if_etag: Option<String>,
        view: Option<DriveView>,
    ) -> Result<()> {
        self.volume_delete(path, if_etag, view).await
    }

    #[deprecated(note = "renamed to `volume_history` (Cua Volume); removed after this release")]
    pub async fn drive_history(
        &self,
        path: String,
        view: Option<DriveView>,
    ) -> Result<Vec<DriveVersion>> {
        self.volume_history(path, view).await
    }

    #[deprecated(note = "renamed to `volume_restore` (Cua Volume); removed after this release")]
    pub async fn drive_restore(
        &self,
        path: String,
        version: String,
        view: Option<DriveView>,
    ) -> Result<DriveObject> {
        self.volume_restore(path, version, view).await
    }

    #[deprecated(note = "renamed to `volume_grant` (Cua Volume); removed after this release")]
    pub async fn drive_grant(
        &self,
        principal: String,
        prefix: String,
        mode: String,
        expires_in_secs: Option<u64>,
        note: Option<String>,
    ) -> Result<DriveGrant> {
        self.volume_grant(principal, prefix, mode, expires_in_secs, note)
            .await
    }

    #[deprecated(note = "renamed to `volume_revoke` (Cua Volume); removed after this release")]
    pub async fn drive_revoke(&self, grant_id: String) -> Result<DriveGrant> {
        self.volume_revoke(grant_id).await
    }

    #[deprecated(note = "renamed to `volume_grants` (Cua Volume); removed after this release")]
    pub async fn drive_grants(&self, all: bool) -> Result<Vec<DriveGrant>> {
        self.volume_grants(all).await
    }

    #[deprecated(
        note = "renamed to `volume_request_access` (Cua Volume); removed after this release"
    )]
    pub async fn drive_request_access(
        &self,
        agent: String,
        prefix: String,
        mode: String,
        reason: Option<String>,
        in_space: Option<String>,
    ) -> Result<DriveAccessRequest> {
        self.volume_request_access(agent, prefix, mode, reason, in_space)
            .await
    }

    #[deprecated(note = "renamed to `volume_requests` (Cua Volume); removed after this release")]
    pub async fn drive_requests(&self) -> Result<Vec<DriveAccessRequest>> {
        self.volume_requests().await
    }

    #[deprecated(note = "renamed to `volume_approve` (Cua Volume); removed after this release")]
    pub async fn drive_approve(
        &self,
        request_id: String,
        expires_in_secs: Option<u64>,
    ) -> Result<DriveGrant> {
        self.volume_approve(request_id, expires_in_secs).await
    }

    #[deprecated(note = "renamed to `volume_deny` (Cua Volume); removed after this release")]
    pub async fn drive_deny(&self, request_id: String) -> Result<()> {
        self.volume_deny(request_id).await
    }

    #[deprecated(note = "renamed to `volume_storage` (Cua Volume); removed after this release")]
    pub async fn drive_storage(&self) -> Result<DriveStorage> {
        self.volume_storage().await
    }

    #[deprecated(note = "renamed to `volume_storage_set` (Cua Volume); removed after this release")]
    pub async fn drive_storage_set(&self, update: DriveStorageUpdate) -> Result<DriveStorageCheck> {
        self.volume_storage_set(update).await
    }

    #[deprecated(
        note = "renamed to `volume_mount_status` (Cua Volume); removed after this release"
    )]
    pub async fn drive_mount_status(&self) -> Result<DriveMountStatus> {
        self.volume_mount_status().await
    }

    #[deprecated(note = "renamed to `volume_mount` (Cua Volume); removed after this release")]
    pub async fn drive_mount(&self) -> Result<DriveMountStatus> {
        self.volume_mount().await
    }

    #[deprecated(note = "renamed to `volume_unmount` (Cua Volume); removed after this release")]
    pub async fn drive_unmount(&self) -> Result<DriveMountStatus> {
        self.volume_unmount().await
    }

    #[deprecated(note = "renamed to `volume_sync_status` (Cua Volume); removed after this release")]
    pub async fn drive_sync_status(&self) -> Result<DriveSyncStatus> {
        self.volume_sync_status().await
    }

    #[deprecated(note = "renamed to `volume_sync_events` (Cua Volume); removed after this release")]
    pub async fn drive_sync_events(
        &self,
        since_seq: Option<u64>,
        wait_ms: Option<u32>,
    ) -> Result<DriveSyncEvents> {
        self.volume_sync_events(since_seq, wait_ms).await
    }

    #[deprecated(
        note = "renamed to `volume_sync_resolve` (Cua Volume); removed after this release"
    )]
    pub async fn drive_sync_resolve(&self, path: String) -> Result<()> {
        self.volume_sync_resolve(path).await
    }

    #[deprecated(note = "renamed to `volume_cache_stats` (Cua Volume); removed after this release")]
    pub async fn drive_cache_stats(&self) -> Result<DriveCacheStats> {
        self.volume_cache_stats().await
    }

    #[deprecated(note = "renamed to `volume_cache_set` (Cua Volume); removed after this release")]
    pub async fn drive_cache_set(&self, capacity_bytes: u64) -> Result<DriveCacheStats> {
        self.volume_cache_set(capacity_bytes).await
    }

    #[deprecated(note = "renamed to `volume_cache_clear` (Cua Volume); removed after this release")]
    pub async fn drive_cache_clear(&self) -> Result<DriveCacheStats> {
        self.volume_cache_clear().await
    }

    #[deprecated(note = "renamed to `volume_audit` (Cua Volume); removed after this release")]
    pub async fn drive_audit(&self, limit: Option<u32>) -> Result<DriveAudit> {
        self.volume_audit(limit).await
    }
}

/// The thumbnail cache's format word (`jpeg`, `png`, `webp`).
fn thumbnail_format(word: &str) -> crate::types::ImageFormat {
    match word {
        "png" => crate::types::ImageFormat::Png,
        "webp" => crate::types::ImageFormat::Webp,
        _ => crate::types::ImageFormat::Jpeg,
    }
}
