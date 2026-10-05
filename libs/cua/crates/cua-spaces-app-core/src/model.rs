// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Space model every shell renders. JSON field names match the webview's
//! `src/model/types.ts` (camelCase, lowercase enum words), so the Tauri
//! frontend passes its objects through unchanged.

use serde::{Deserialize, Serialize};

/// Operating system a Space runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpaceOs {
    /// macOS.
    Macos,
    /// Windows.
    Windows,
    /// Linux.
    Linux,
}

impl SpaceOs {
    /// "macOS", "Windows", "Linux".
    pub fn label(self) -> &'static str {
        match self {
            SpaceOs::Macos => "macOS",
            SpaceOs::Windows => "Windows",
            SpaceOs::Linux => "Linux",
        }
    }

    /// The JSON word.
    pub fn as_str(self) -> &'static str {
        match self {
            SpaceOs::Macos => "macos",
            SpaceOs::Windows => "windows",
            SpaceOs::Linux => "linux",
        }
    }

    /// Parses the JSON word.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "macos" => Some(SpaceOs::Macos),
            "windows" => Some(SpaceOs::Windows),
            "linux" => Some(SpaceOs::Linux),
            _ => None,
        }
    }
}

/// Lifecycle status of a Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpaceStatus {
    /// The host machine itself.
    Local,
    /// Reachable, session active.
    Running,
    /// An agent waits for approval.
    Approval,
    /// Unreachable or suspended.
    Suspended,
    /// Being created.
    Provisioning,
    /// Being deleted (the user confirmed; the SDK is removing it).
    Deleting,
}

impl SpaceStatus {
    /// The status word shown next to a Space.
    pub fn label(self) -> &'static str {
        match self {
            SpaceStatus::Local => "Local",
            SpaceStatus::Running => "Running",
            SpaceStatus::Approval => "Needs approval",
            SpaceStatus::Suspended => "Suspended",
            SpaceStatus::Provisioning => "Starting",
            SpaceStatus::Deleting => "Deleting\u{2026}",
        }
    }

    /// Rows for Spaces in these states are drawn at full opacity; others dim.
    pub fn is_live(self) -> bool {
        matches!(
            self,
            SpaceStatus::Running | SpaceStatus::Local | SpaceStatus::Provisioning
        )
    }
}

/// Synthetic thumbnail scene (fixtures and placeholders only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThumbnailScene {
    /// A macOS desktop.
    MacDesktop,
    /// A Windows desktop.
    WindowsDesktop,
    /// A Linux terminal.
    LinuxTerminal,
    /// A notes app.
    Notes,
    /// A browser.
    Browser,
    /// Nothing.
    Blank,
}

/// Where a Space runs: the SDK's location words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpaceProvider {
    /// Cua Cloud.
    Cloud,
    /// This Mac.
    Local,
    /// A machine added by address.
    Direct,
    /// A machine of the signed-in account through the relay.
    Relay,
}

impl SpaceProvider {
    /// The JSON word.
    pub fn as_str(self) -> &'static str {
        match self {
            SpaceProvider::Cloud => "cloud",
            SpaceProvider::Local => "local",
            SpaceProvider::Direct => "direct",
            SpaceProvider::Relay => "relay",
        }
    }

    /// A known location word.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "cloud" => Some(SpaceProvider::Cloud),
            "local" => Some(SpaceProvider::Local),
            "direct" => Some(SpaceProvider::Direct),
            "relay" => Some(SpaceProvider::Relay),
            _ => None,
        }
    }
}

/// What the SDK reported about a registered Space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceSdkRef {
    /// spacesd feature names.
    pub features: Vec<String>,
    /// spacesd version at the last handshake.
    pub spacesd_version: String,
    /// Answered the last bounded connect.
    pub reachable: bool,
    /// Why not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One Space as the shells show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Space {
    /// Sandbox ref (`cloud:<name>`, `local:<name>`, `direct:<host:port>`, ...).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Operating system.
    pub os: SpaceOs,
    /// Status.
    pub status: SpaceStatus,
    /// One short status detail.
    pub detail: String,
    /// Epoch ms of the last focus.
    pub last_used_at: i64,
    /// Epoch ms it was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// Placeholder scene.
    pub scene: ThumbnailScene,
    /// Group (cloud namespace or location word).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fleet_id: Option<String>,
    /// Fixture machine size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    /// Fixture region.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Where it runs (cloud when omitted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<SpaceProvider>,
    /// SDK facts, for Spaces from the live registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk: Option<SpaceSdkRef>,
    /// The guest OS product or distribution ("Ubuntu", "macOS"), when the
    /// Space reported one: it picks the OS icon ([`crate::notch::os_icon`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_name: Option<String>,
    /// The guest's full OS string ("Ubuntu 24.04.3 LTS"), when the Space
    /// reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_pretty_name: Option<String>,
    /// The image it runs ("ghcr.io/trycua/linux:24.04"), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// The digest of the variant that runs, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_digest: Option<String>,
    /// While it is being created: how far along, from the SDK's create
    /// progress ([`crate::spaces::creating`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<SpaceProgress>,
    /// Container or virtual machine, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SpaceKind>,
    /// The guest's CPU architecture (`arm64`, `amd64`), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    /// For a Space one of your machines provides: that machine's relay id
    /// (the sidebar nests it under the machine).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// That machine's name, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
    /// Whether and how it turns off and on (the SDK's `power`), with a
    /// power action in flight ([`crate::spaces::creating::compose`]).
    /// `None`: it cannot (a cloud Space, one added by address).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<SpacePower>,
    /// For a Space in your cloud: the provider word (`aws`, `gcp`,
    /// `modal`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
    /// Its account and region in words ("AWS · us-west-2").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_place: Option<String>,
    /// Where Delete permanently can delete it: `here`, `host:<machine>`
    /// (that machine created it) or `elsewhere` (another device did).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_delete: Option<String>,
}

/// How a Space turns off: the SDK's `power` words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerControl {
    /// Suspended in memory; it resumes where it was.
    Suspend,
    /// Stopped with its disk kept; it boots again.
    Stop,
}

impl PowerControl {
    /// Parses the SDK's word (`suspend`, `stop`).
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim() {
            "suspend" => Some(PowerControl::Suspend),
            "stop" => Some(PowerControl::Stop),
            _ => None,
        }
    }
}

/// A Space's power: how it turns off, whether it is off now, and what a
/// power action in flight is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpacePower {
    /// How it turns off.
    pub control: PowerControl,
    /// It is off: suspended or stopped as the SDK recorded it, or not
    /// answering with no recorded state (a Space one of your machines
    /// provides).
    pub off: bool,
    /// Being turned on (`true`) or off (`false`) now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turning_on: Option<bool>,
    /// Why the last power action failed, until the next one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// How far a Space being created has come.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceProgress {
    /// The SDK's phase word (`preparing`, `pulling`, `booting`, ...).
    pub phase: String,
    /// Overall progress in thousandths.
    pub permille: u32,
    /// The phase in words ("Starting…", "Downloading image…"), or "Failed".
    pub label: String,
    /// Why the create failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// It failed because the account is out of Cua Cloud credit: the
    /// website billing page "Add credit" opens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit_url: Option<String>,
    /// While it downloads: "4.2 of 22.1 GB · 85 MB/s · about 4 min"
    /// ([`crate::spaces::creating::transfer_text`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer: Option<String>,
    /// Cancel can stop it now.
    #[serde(default)]
    pub cancellable: bool,
    /// Cancel was pressed and the create is being cleaned up.
    #[serde(default)]
    pub cancelling: bool,
}

/// One row of the SDK's `list_spaces`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceRow {
    /// Sandbox ref.
    pub id: String,
    /// Name spacesd reported.
    pub name: String,
    /// Location word.
    pub provider: String,
    /// spacesd version.
    #[serde(default)]
    pub spacesd_version: String,
    /// spacesd features.
    #[serde(default)]
    pub features: Vec<String>,
    /// RFC 3339 time it was added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
    /// Operating system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<SpaceOs>,
    /// OS product or distribution spacesd reported ("Ubuntu").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_name: Option<String>,
    /// The guest's full OS string ("Ubuntu 24.04.3 LTS"), when the Space
    /// reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os_pretty_name: Option<String>,
    /// The image it runs ("ghcr.io/trycua/linux:24.04"), when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// The digest of the variant that runs, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_digest: Option<String>,
    /// Container or virtual machine, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SpaceKind>,
    /// The guest's CPU architecture, when known (any spelling: `arm64`,
    /// `aarch64`, `amd64`, `x86_64`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    /// Answered the last connect.
    pub reachable: bool,
    /// Why not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// For a Space one of your machines provides: that machine's relay id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// That machine's name, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_name: Option<String>,
    /// How it turns off and on (`suspend`, `stop`); absent when it cannot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<String>,
    /// `running`, `suspended` or `stopped` as the SDK recorded it; absent
    /// when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_state: Option<String>,
    /// For a Space in your cloud: the provider word (`aws`, `gcp`,
    /// `modal`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud: Option<String>,
    /// Its account and region in words ("AWS · us-west-2").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_place: Option<String>,
    /// Where Delete permanently can delete it: `here`, `host:<machine>`
    /// (that machine created it) or `elsewhere` (another device did).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_delete: Option<String>,
}

/// `arm64` or `amd64` for any spelling of a CPU architecture (`aarch64`,
/// `x86_64`, `x64`, ...); `None` for others and empty text.
pub fn normalize_arch(arch: &str) -> Option<&'static str> {
    match arch.trim().to_ascii_lowercase().as_str() {
        "arm64" | "aarch64" | "arm" | "arm64e" => Some("arm64"),
        "amd64" | "x86_64" | "x86-64" | "x64" => Some("amd64"),
        _ => None,
    }
}

/// The architecture in words: "ARM" or "x64".
pub fn arch_label(arch: &str) -> Option<&'static str> {
    normalize_arch(arch).map(|a| if a == "arm64" { "ARM" } else { "x64" })
}

/// This machine's CPU architecture, as the image catalog spells it (`arm64`,
/// `amd64`): what a native shell passes as `host_arch` (the Swift app
/// through the SDK's `app_host_arch`, the Tauri shell's `host_arch`
/// command). Not meaningful in the wasm core, which never calls it.
pub fn this_host_arch() -> &'static str {
    normalize_arch(std::env::consts::ARCH).unwrap_or(std::env::consts::ARCH)
}

/// The SF Symbol of a warning after a value (an emulated Space's
/// Architecture).
pub const WARNING_SYMBOL: &str = "exclamationmark.triangle";

/// The platform a Space of an image built for `platforms` runs (the one
/// rule for the New Space wizard, a pending create and its detail):
/// locally the host's when the image has it, else what it has (emulated),
/// unknown while the host's is; in the cloud x64 when the image has it.
/// `None` when the image lists no platform.
pub fn run_arch(platforms: &[String], local: bool, host_arch: Option<&str>) -> Option<String> {
    let listed: Vec<&'static str> = platforms.iter().filter_map(|a| normalize_arch(a)).collect();
    let want = if local {
        normalize_arch(host_arch?)?
    } else {
        "amd64"
    };
    if listed.contains(&want) {
        Some(want.to_string())
    } else {
        listed.first().map(|a| a.to_string())
    }
}

/// The tooltip of a local Space that runs `arch` on a host of `host_arch`
/// when they differ ("Emulated on this Mac\u{2019}s ARM processor.
/// Performance may be degraded."); `None` in the cloud, when they match or
/// when either is unknown.
pub fn emulation_warning(
    local: bool,
    host_arch: Option<&str>,
    arch: Option<&str>,
) -> Option<String> {
    let host = normalize_arch(host_arch?)?;
    let arch = normalize_arch(arch?)?;
    (local && host != arch).then(|| {
        format!(
            "Emulated on this Mac\u{2019}s {} processor. Performance may be degraded.",
            arch_label(host).unwrap_or(host)
        )
    })
}

/// The apps offer Cua Cloud for new Spaces. Off: the apps create Spaces on
/// this Mac and on machines added by address, and the New Space wizard
/// shows "Your cloud" as coming soon. The cua SDK and CLI still create
/// cloud sandboxes, and cloud Spaces that already exist still list.
pub const CLOUD_SPACES_OFFERED: bool = false;

/// Where new Spaces can be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Location {
    /// Cua Cloud (the SDK's `on="cloud"`). The apps do not offer it for new
    /// Spaces ([`CLOUD_SPACES_OFFERED`]); existing cloud Spaces still list.
    Cloud,
    /// This Mac.
    Local,
    /// The user's own cloud account (AWS, Google Cloud, Modal), connected
    /// with `cua cloud connect`: the SDK's `on` is the provider's word
    /// (`aws`), which the wizard carries beside it.
    Yours,
    /// One of the user's machines that provides Spaces (a relay host, or
    /// one added by its Tailscale or LAN address): the SDK's `on` is
    /// `host:<machine>`, which the wizard carries beside it.
    Host,
}

impl Location {
    /// "Cua Cloud" / "This Mac" / "Your cloud" / "Your machine".
    pub fn label(self) -> &'static str {
        match self {
            Location::Cloud => "Cua Cloud",
            Location::Local => "This Mac",
            Location::Yours => "Your cloud",
            Location::Host => "Your machine",
        }
    }

    /// Where an image goes when it cannot run here: this Mac, else Cua
    /// Cloud.
    pub fn other(self) -> Self {
        match self {
            Location::Cloud | Location::Yours | Location::Host => Location::Local,
            Location::Local => Location::Cloud,
        }
    }

    /// The SDK's `on` word (`yours` and `host` are not ones: the
    /// provider's word and `host:<machine>` are).
    pub fn as_str(self) -> &'static str {
        match self {
            Location::Cloud => "cloud",
            Location::Local => "local",
            Location::Yours => "yours",
            Location::Host => "host",
        }
    }
}

/// Container or virtual machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpaceKind {
    /// A container.
    Container,
    /// A virtual machine.
    Vm,
}

impl SpaceKind {
    /// "Container" / "Virtual machine".
    pub fn label(self) -> &'static str {
        match self {
            SpaceKind::Container => "Container",
            SpaceKind::Vm => "Virtual machine",
        }
    }

    /// The SDK's `kind` word.
    pub fn as_str(self) -> &'static str {
        match self {
            SpaceKind::Container => "container",
            SpaceKind::Vm => "vm",
        }
    }
}

/// The engine that runs a Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    /// Let the SDK pick.
    Auto,
    /// gVisor.
    Gvisor,
    /// runc.
    Runc,
    /// QEMU.
    Qemu,
    /// Lume.
    Lume,
    /// KubeVirt.
    Kubevirt,
}

impl Runtime {
    /// "Automatic", "gVisor", ...
    pub fn label(self) -> &'static str {
        match self {
            Runtime::Auto => "Automatic",
            Runtime::Gvisor => "gVisor",
            Runtime::Runc => "runc",
            Runtime::Qemu => "QEMU",
            Runtime::Lume => "Lume",
            Runtime::Kubevirt => "KubeVirt",
        }
    }

    /// The SDK's `runtime` word.
    pub fn as_str(self) -> &'static str {
        match self {
            Runtime::Auto => "auto",
            Runtime::Gvisor => "gvisor",
            Runtime::Runc => "runc",
            Runtime::Qemu => "qemu",
            Runtime::Lume => "lume",
            Runtime::Kubevirt => "kubevirt",
        }
    }
}
