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
    /// The host has not reported a recognized operating system.
    #[serde(other)]
    Unknown,
}

impl SpaceOs {
    /// The system this build runs on, for the words of a view a native
    /// shell asks the library for (the Electron shell on Windows and Linux).
    /// macOS in the wasm core, whose callers name theirs (`hostOs`).
    pub fn this_host() -> Self {
        if cfg!(target_os = "windows") {
            SpaceOs::Windows
        } else if cfg!(target_os = "linux") {
            SpaceOs::Linux
        } else {
            SpaceOs::Macos
        }
    }

    /// "macOS", "Windows", "Linux".
    pub fn label(self) -> &'static str {
        match self {
            SpaceOs::Macos => "macOS",
            SpaceOs::Windows => "Windows",
            SpaceOs::Linux => "Linux",
            SpaceOs::Unknown => "Unknown",
        }
    }

    /// The JSON word.
    pub fn as_str(self) -> &'static str {
        match self {
            SpaceOs::Macos => "macos",
            SpaceOs::Windows => "windows",
            SpaceOs::Linux => "linux",
            SpaceOs::Unknown => "unknown",
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

    /// What the app calls the computer it runs on, when that runs this
    /// system: "This Mac", "This PC", "This computer".
    pub fn this_machine(self) -> &'static str {
        match self {
            SpaceOs::Macos => "This Mac",
            SpaceOs::Windows => "This PC",
            SpaceOs::Linux | SpaceOs::Unknown => "This computer",
        }
    }

    /// [`Self::this_machine`] mid-sentence: "this Mac", "this PC",
    /// "this computer".
    pub fn this_machine_lower(self) -> &'static str {
        match self {
            SpaceOs::Macos => "this Mac",
            SpaceOs::Windows => "this PC",
            SpaceOs::Linux | SpaceOs::Unknown => "this computer",
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
            SpaceProvider::Local => "local",
            SpaceProvider::Direct => "direct",
            SpaceProvider::Relay => "relay",
        }
    }

    /// A known location word.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
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
    /// Who it belongs to, who else has it and what you may do with it
    /// ([`crate::spaces::access`]). Absent reads as yours with nobody else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<SpaceAccess>,
}

/// Whose a Space is, as the Spaces page groups it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpaceAccessKind {
    /// You own it (shared or not).
    Mine,
    /// Someone shared it with you.
    WithMe,
    /// A team's Space.
    Team,
}

/// What someone may do in a Space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpaceAccessRole {
    /// It is theirs.
    Owner,
    /// Full use.
    Editor,
    /// Watch only.
    Viewer,
}

/// Someone (or a team) a Space of yours is shared with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpacePerson {
    /// An email or account id as shared, or `team:<key>`.
    pub who: String,
    /// A name to show, when known (a team's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Editor or viewer.
    pub role: SpaceAccessRole,
    /// A whole team.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub team: bool,
}

/// Who a Space belongs to, who else has it, and what you may do with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceAccess {
    /// Yours, shared with you, or a team's.
    pub kind: SpaceAccessKind,
    /// What you may do.
    pub role: SpaceAccessRole,
    /// Shared with you or a team's: the owner's name (display name, else
    /// the email's local part).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_name: Option<String>,
    /// The owner's email, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    /// A team's: the team's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_name: Option<String>,
    /// The meta line: "This Mac", "Sam's Mac mini", "QA".
    pub place: String,
    /// Yours: who has it.
    #[serde(default)]
    pub people: Vec<SpacePerson>,
    /// Turn it on and off.
    pub can_power: bool,
    /// Teleport an app into it.
    pub can_teleport: bool,
    /// Share it.
    pub can_share: bool,
    /// Rename it.
    pub can_rename: bool,
    /// Delete it (yours, or a team's you made).
    pub can_delete: bool,
    /// Leave it (shared with you, or a team's you did not make).
    pub can_leave: bool,
}

/// What the SDK says about who owns a relay Space and who else has it
/// ([`SpaceRow::sharing`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceSharingFacts {
    /// The owner's account id.
    #[serde(default)]
    pub owner_id: String,
    /// The owner's email, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    /// The owner's display name, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_name: Option<String>,
    /// `owner`, `editor` or `viewer`.
    pub role: String,
    /// The team you reach it through (its key).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// That team's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_name: Option<String>,
    /// Owner view: who may use it.
    #[serde(default)]
    pub editors: Vec<String>,
    /// Owner view: who may only watch.
    #[serde(default)]
    pub viewers: Vec<String>,
    /// Owner view: the teams it is shared with.
    #[serde(default)]
    pub teams: Vec<SpaceTeamFact>,
}

/// A team a Space is shared with ([`SpaceSharingFacts::teams`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceTeamFact {
    /// The team's key.
    pub key: String,
    /// Its name.
    pub name: String,
    /// `editor` or `viewer`.
    pub role: String,
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
    /// Why it stopped without being asked to, in plain words ("out of
    /// memory"), while it is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
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
    /// The app is running the create a second time by itself, after a
    /// failure that usually passes (the label says "Retrying…" while it
    /// prepares).
    #[serde(default)]
    pub retrying: bool,
    /// It failed the same way twice in a row (same cause, same machine,
    /// same image): the fix to show instead of a plain Try again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<CreateFix>,
}

/// What a fix button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CreateFixAction {
    /// Copies `value` (a Terminal command) to the clipboard.
    CopyCommand,
    /// Opens `value` (a help page) in the browser.
    OpenUrl,
    /// Opens the failed create's details (the diagnostic report to review).
    Details,
}

/// The fix for a create that keeps failing the same way: one plain
/// sentence and one button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateFix {
    /// What is wrong and what to do, in plain words.
    pub text: String,
    /// The button's words.
    pub label: String,
    /// What the button does.
    pub action: CreateFixAction,
    /// The command to copy or the page to open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// One row of the SDK's `list_spaces`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceRow {
    /// Sandbox ref.
    pub id: String,
    /// Name spacesd reported.
    pub name: String,
    /// The name it was given with Rename (the SDK's
    /// `SpaceInfo.display_name`): shown in place of `name` when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
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
    /// Why a local Space stopped without being asked to, in plain words
    /// ("out of memory", "killed", "exited with code 1"), as the SDK
    /// recorded it; absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// A relay Space: who owns it and who else has it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sharing: Option<SpaceSharingFacts>,
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

/// The architecture in words on a host of `os`: "ARM" or "x64" on a Mac
/// (as [`arch_label`]), "arm64" or "x64" on Windows and Linux, where
/// "ARM" alone reads as 32-bit.
pub fn arch_label_on(os: SpaceOs, arch: &str) -> Option<&'static str> {
    match os {
        SpaceOs::Macos => arch_label(arch),
        _ => normalize_arch(arch).map(|a| if a == "arm64" { "arm64" } else { "x64" }),
    }
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
    emulation_warning_on(SpaceOs::Macos, local, host_arch, arch)
}

/// [`emulation_warning`] on a host of `os` ("Emulated on this PC\u{2019}s
/// arm64 processor. ...").
pub fn emulation_warning_on(
    os: SpaceOs,
    local: bool,
    host_arch: Option<&str>,
    arch: Option<&str>,
) -> Option<String> {
    let host = normalize_arch(host_arch?)?;
    let arch = normalize_arch(arch?)?;
    (local && host != arch).then(|| {
        format!(
            "Emulated on {}\u{2019}s {} processor. Performance may be degraded.",
            os.this_machine_lower(),
            arch_label_on(os, host).unwrap_or(host)
        )
    })
}

/// Where new Spaces can be created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Location {
    /// This Mac.
    Local,
    /// The user's own cloud account (AWS, Google Cloud, Hetzner, Modal), connected
    /// with `cua cloud connect`: the SDK's `on` is the provider's word
    /// (`aws`), which the wizard carries beside it.
    Yours,
    /// One of the user's machines that provides Spaces (a relay host, or
    /// one added by its Tailscale or LAN address): the SDK's `on` is
    /// `host:<machine>`, which the wizard carries beside it.
    Host,
    /// The least-loaded of the user's machines that can take it: this Mac
    /// or one of their machines that provides Spaces (the SDK's
    /// `on="auto"`). Offered only while the user has such a machine, and
    /// never a default.
    Auto,
}

impl Location {
    /// "This Mac" / "Your cloud" / "Your computer" / "Auto".
    pub fn label(self) -> &'static str {
        self.label_on(SpaceOs::Macos)
    }

    /// [`Self::label`] in a shell on `os`: local is "This PC" on Windows,
    /// "This computer" on Linux.
    pub fn label_on(self, os: SpaceOs) -> &'static str {
        match self {
            Location::Local => os.this_machine(),
            Location::Yours => "Your cloud",
            Location::Host => "Your computer",
            Location::Auto => "Auto",
        }
    }

    /// The SDK's `on` word (`yours` and `host` are not ones: the
    /// provider's word and `host:<machine>` are).
    pub fn as_str(self) -> &'static str {
        match self {
            Location::Local => "local",
            Location::Yours => "yours",
            Location::Host => "host",
            Location::Auto => "auto",
        }
    }
}

/// Container, virtual machine or microVM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpaceKind {
    /// A container.
    Container,
    /// A virtual machine.
    Vm,
    /// A microVM: a container image booted with its own kernel
    /// (Firecracker on Linux with KVM; on a Mac, Virtualization.framework
    /// through cua-vz or Apple's Containerization).
    Microvm,
}

impl SpaceKind {
    /// "Container" / "Virtual machine" / "MicroVM".
    pub fn label(self) -> &'static str {
        match self {
            SpaceKind::Container => "Container",
            SpaceKind::Vm => "Virtual machine",
            SpaceKind::Microvm => "MicroVM",
        }
    }

    /// The SDK's `kind` word.
    pub fn as_str(self) -> &'static str {
        match self {
            SpaceKind::Container => "container",
            SpaceKind::Vm => "vm",
            SpaceKind::Microvm => "microvm",
        }
    }
}

/// The engine that runs a Space.
///
/// JSON is the SDK's plain `runtime` word: the built-in engines' words, or
/// the name of an external runtime provider ([`Runtime::Other`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
    /// Firecracker: a microVM on Linux with KVM.
    Firecracker,
    /// Virtualization.framework through cua's own monitor (`cua-vz`): a
    /// microVM on an Apple silicon Mac.
    Vz,
    /// Apple's Containerization: a microVM on a Mac with macOS 26.
    Containerization,
    /// An external runtime provider installed on this machine, by its name
    /// (the `<name>` of its `cua-runtime-<name>` executable).
    Other(String),
}

/// Whether `word` has the shape of an external runtime provider's name:
/// `[a-z0-9][a-z0-9-]{0,31}`.
pub fn valid_runtime_word(word: &str) -> bool {
    let b = word.as_bytes();
    (1..=32).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

impl Runtime {
    /// "Automatic", "gVisor", ...; for an external runtime provider, the
    /// name it gives itself while registered
    /// ([`crate::wizard::set_external_runtimes`]), else its runtime word.
    pub fn label(&self) -> String {
        match self {
            Runtime::Auto => "Automatic".into(),
            Runtime::Gvisor => "gVisor".into(),
            Runtime::Runc => "runc".into(),
            Runtime::Qemu => "QEMU".into(),
            Runtime::Lume => "Lume".into(),
            Runtime::Firecracker => "Firecracker".into(),
            Runtime::Vz => "Virtualization.framework".into(),
            Runtime::Containerization => "Apple Containerization".into(),
            Runtime::Other(n) => crate::wizard::external_runtime_label(n),
        }
    }

    /// The SDK's `runtime` word.
    pub fn as_str(&self) -> &str {
        match self {
            Runtime::Auto => "auto",
            Runtime::Gvisor => "gvisor",
            Runtime::Runc => "runc",
            Runtime::Qemu => "qemu",
            Runtime::Lume => "lume",
            Runtime::Firecracker => "firecracker",
            Runtime::Vz => "vz",
            Runtime::Containerization => "containerization",
            Runtime::Other(n) => n,
        }
    }

    /// Parses the SDK's `runtime` word: a built-in engine's, else an
    /// external runtime provider's name. `None` for a word that is neither.
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "auto" => Runtime::Auto,
            "gvisor" => Runtime::Gvisor,
            "runc" => Runtime::Runc,
            "qemu" => Runtime::Qemu,
            "lume" => Runtime::Lume,
            "firecracker" => Runtime::Firecracker,
            "vz" => Runtime::Vz,
            "containerization" => Runtime::Containerization,
            w if valid_runtime_word(w) => Runtime::Other(w.to_string()),
            _ => return None,
        })
    }
}

impl Serialize for Runtime {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Runtime {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let word = String::deserialize(deserializer)?;
        Runtime::parse(&word)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown runtime `{word}`")))
    }
}

#[cfg(test)]
mod runtime_tests {
    use super::*;

    #[test]
    fn runtime_words_round_trip() {
        for (word, rt) in [
            ("auto", Runtime::Auto),
            ("gvisor", Runtime::Gvisor),
            ("runc", Runtime::Runc),
            ("qemu", Runtime::Qemu),
            ("lume", Runtime::Lume),
            ("firecracker", Runtime::Firecracker),
            ("vz", Runtime::Vz),
            ("containerization", Runtime::Containerization),
            ("acme", Runtime::Other("acme".into())),
            ("acme-2", Runtime::Other("acme-2".into())),
        ] {
            let json = format!("\"{word}\"");
            assert_eq!(serde_json::from_str::<Runtime>(&json).unwrap(), rt);
            assert_eq!(serde_json::to_string(&rt).unwrap(), json);
            assert_eq!(rt.as_str(), word);
        }
    }

    #[test]
    fn runtime_rejects_words_no_provider_can_have() {
        for word in ["", "Acme", "-acme", "acme_vm", "a b", &"a".repeat(33)] {
            assert!(
                serde_json::from_str::<Runtime>(&format!("\"{word}\"")).is_err(),
                "{word}"
            );
        }
        assert!(valid_runtime_word(&"a".repeat(32)));
    }

    #[test]
    fn an_unregistered_provider_is_labelled_by_its_word() {
        assert_eq!(Runtime::Gvisor.label(), "gVisor");
        assert_eq!(Runtime::Other("zeta".into()).label(), "zeta");
    }
}
