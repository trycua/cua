// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "This machine": the roster entry that is always present, its one-line
//! sharing summary, its page (how it is shared, who is connected, the
//! permissions left to grant, the buttons), and the host setup form (state,
//! fields, copy, validation). Both shells validate a setup request here
//! before any service is touched.

use crate::model::{Space, SpaceOs, SpaceStatus};
use crate::spaces::scene_for_os;
use serde::{Deserialize, Serialize};

/// Id of the "This machine" roster entry.
pub const THIS_MACHINE_ID: &str = "this-mac";

/// The parts of the host status the summary reads (`cua-host`'s status).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSummaryInput {
    /// Set up for access.
    pub configured: bool,
    /// Needs attention.
    #[serde(default)]
    pub error: Option<String>,
    /// The host service runs.
    pub service_running: bool,
    /// Sharing is on.
    pub sharing: bool,
    /// Connected clients.
    pub clients: u32,
    /// `relay` or `direct`.
    #[serde(default)]
    pub mode: Option<String>,
    /// Relay presence.
    #[serde(default)]
    pub online: Option<bool>,
}

/// One line for the entry.
pub fn host_summary(status: Option<&HostSummaryInput>) -> String {
    let Some(s) = status.filter(|s| s.configured) else {
        return "Set up for access".into();
    };
    if let Some(e) = s.error.as_deref().filter(|e| !e.is_empty()) {
        return format!("Needs attention \u{b7} {e}");
    }
    if !s.service_running {
        return "Host service stopped".into();
    }
    if !s.sharing {
        return "Not sharing".into();
    }
    let via = if s.mode.as_deref() == Some("direct") {
        "Direct"
    } else {
        "Relay"
    };
    if s.clients > 0 {
        return connected_detail(s.clients);
    }
    if s.online == Some(false) {
        return format!("{via} \u{b7} offline");
    }
    format!("Sharing \u{b7} {via}")
}

/// "Sharing · N connected": the entry's detail while someone is connected.
pub fn connected_detail(n: u32) -> String {
    format!("{CONNECTED_PREFIX}{n}{CONNECTED_SUFFIX}")
}

const CONNECTED_PREFIX: &str = "Sharing \u{b7} ";
const CONNECTED_SUFFIX: &str = " connected";

/// How many are connected to this machine right now, read back from the
/// "This machine" roster entry the core built ([`connected_detail`]); 0 for
/// any other Space. The notch indicator uses it, so it needs no wiring in
/// either shell beyond the roster they already pass.
pub fn connected_now(space: &Space) -> u32 {
    if space.id != THIS_MACHINE_ID {
        return 0;
    }
    space
        .detail
        .strip_prefix(CONNECTED_PREFIX)
        .and_then(|d| d.strip_suffix(CONNECTED_SUFFIX))
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// This machine's dot: green (`Local`) only while it is set up and sharing
/// for access; orange (`Approval`) when the host needs attention; gray
/// (`Suspended`) otherwise (not set up, stopped, not sharing, offline).
pub fn this_machine_status(status: Option<&HostSummaryInput>) -> SpaceStatus {
    let Some(s) = status.filter(|s| s.configured) else {
        return SpaceStatus::Suspended;
    };
    if s.error.as_deref().is_some_and(|e| !e.is_empty()) {
        return SpaceStatus::Approval;
    }
    if s.service_running && s.sharing && s.online != Some(false) {
        SpaceStatus::Local
    } else {
        SpaceStatus::Suspended
    }
}

/// The roster entry for this machine.
pub fn this_machine_space(status: Option<&HostSummaryInput>, now: i64, os: SpaceOs) -> Space {
    Space {
        id: THIS_MACHINE_ID.into(),
        name: "This machine".into(),
        os,
        status: this_machine_status(status),
        detail: host_summary(status),
        last_used_at: now,
        started_at: None,
        scene: scene_for_os(os),
        fleet_id: None,
        size: None,
        region: None,
        provider: None,
        sdk: None,
        os_name: None,
        progress: None,
        os_pretty_name: None,
        image: None,
        image_digest: None,
        kind: None,
        arch: None,
        host: None,
        host_name: None,
        power: None,
        cloud: None,
        cloud_place: None,
        cloud_delete: None,
    }
}

/// `ip:port` or `host:port` (port required) for the direct mode.
pub fn looks_like_listen(value: &str) -> bool {
    let v = value.trim();
    let Some(idx) = v.rfind(':') else {
        return false;
    };
    let (host, port) = (&v[..idx], &v[idx + 1..]);
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let port: u32 = port.parse().unwrap_or(0);
    if port == 0 || port >= 65_536 {
        return false;
    }
    if let Some(inner) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return !inner.is_empty() && inner.bytes().all(|b| b.is_ascii_hexdigit() || b == b':');
    }
    !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// Comma or space separated account ids or emails.
pub fn parse_allow_list(value: &str) -> Vec<String> {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// A connected client, for display.
pub fn client_label(id: &str, email: Option<&str>, name: Option<&str>) -> String {
    name.filter(|s| !s.is_empty())
        .or(email.filter(|s| !s.is_empty()))
        .unwrap_or(id)
        .to_string()
}

/// The host setup form: what both shells send to host setup.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSetupRequest {
    /// `relay` (default) or `direct`.
    pub mode: String,
    /// Relay URL; `None` uses the configured default (https://relay.cua.ai).
    #[serde(default)]
    pub relay_url: Option<String>,
    /// Direct `ip:port` (Advanced).
    #[serde(default)]
    pub direct: Option<String>,
    /// Display name (default: the host name).
    #[serde(default)]
    pub name: Option<String>,
    /// Accounts (ids or emails) allowed besides the owner.
    #[serde(default)]
    pub allow: Option<Vec<String>>,
    /// What the machine is for: `desktop` (share this desktop; the
    /// default) or `spare` (only run Spaces for your other devices).
    #[serde(default)]
    pub profile: Option<String>,
    /// Share this machine's own desktop (overrides the profile).
    #[serde(default)]
    pub share_desktop: Option<bool>,
    /// Create Spaces for your other devices (overrides the profile).
    #[serde(default)]
    pub provide_spaces: Option<bool>,
}

impl HostSetupRequest {
    /// `(share_desktop, provide_spaces)` the request asks for: the
    /// profile's, then each explicit setting.
    pub fn settings(&self) -> (bool, bool) {
        let (mut desktop, mut provide) = match self.profile.as_deref() {
            Some("spare") => (false, true),
            _ => (true, false),
        };
        if let Some(d) = self.share_desktop {
            desktop = d;
        }
        if let Some(p) = self.provide_spaces {
            provide = p;
        }
        (desktop, provide)
    }
}

/// Normalises and checks a setup request before any service is touched.
pub fn validate_setup(mut request: HostSetupRequest) -> Result<HostSetupRequest, String> {
    request.mode = request.mode.trim().to_ascii_lowercase();
    if request.mode.is_empty() {
        request.mode = "relay".into();
    }
    request.name = request
        .name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty());
    request.allow = request
        .allow
        .map(|a| {
            a.into_iter()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|a| !a.is_empty());
    request.profile = match request
        .profile
        .as_deref()
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| !p.is_empty())
    {
        None => None,
        Some(p) if p == "desktop" || p == "spare" => Some(p),
        Some(p) => return Err(format!("unknown profile {p:?} (desktop or spare)")),
    };
    let (desktop, provide) = request.settings();
    if !desktop && !provide {
        return Err("share this desktop, provide Spaces, or both".into());
    }
    if request.mode == "direct" && provide {
        return Err("providing Spaces needs the relay".into());
    }
    if request.mode == "direct" && !desktop {
        return Err("a direct connection always shares this desktop".into());
    }
    match request.mode.as_str() {
        "relay" => {
            request.direct = None;
            if let Some(url) = request.relay_url.as_deref().map(str::trim) {
                if url.is_empty() {
                    request.relay_url = None;
                } else if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err(format!("relay URL must be http(s): {url:?}"));
                } else {
                    request.relay_url = Some(url.trim_end_matches('/').to_string());
                }
            }
            Ok(request)
        }
        "direct" => {
            let direct = request
                .direct
                .as_deref()
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            direct
                .parse::<std::net::SocketAddr>()
                .map_err(|_| format!("direct mode needs ip:port, got {direct:?}"))?;
            request.direct = Some(direct);
            request.relay_url = None;
            Ok(request)
        }
        other => Err(format!("unknown host mode {other:?} (relay or direct)")),
    }
}

/// The relay host setup joins unless the form names another.
pub const DEFAULT_RELAY_URL: &str = "https://relay.cua.ai";
/// The direct mode's default listen address.
pub const DEFAULT_LISTEN: &str = "0.0.0.0:3211";

/// Adds the "This machine" entry to a roster, first (it is always present
/// in the live app; `status` is `None` until the host answers).
pub fn with_this_machine(
    spaces: &[Space],
    status: Option<&HostSummaryInput>,
    now: i64,
    os: SpaceOs,
) -> Vec<Space> {
    let mut out = Vec::with_capacity(spaces.len() + 1);
    out.push(this_machine_space(status, now, os));
    out.extend(spaces.iter().filter(|s| s.id != THIS_MACHINE_ID).cloned());
    out
}

/// Someone connected to this machine (relay presence).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostClient {
    /// Account id.
    pub id: String,
    /// Email.
    #[serde(default)]
    pub email: Option<String>,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Open streams.
    #[serde(default)]
    pub streams: Option<u32>,
}

/// One access to this machine (the host's hash-chained access log).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAccess {
    /// Epoch ms.
    pub at_ms: i64,
    /// How the caller authenticated: `relay`, `viewer` or `token`.
    pub via: String,
    /// Who (a relay-verified account, a viewer, or the token holder).
    pub who: String,
    /// What they used (a service name such as `ProcessService`, or `MCP`).
    pub what: String,
}

/// An OS permission host setup needs (a macOS privacy pane).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPermissionInput {
    /// `screen-recording`, `accessibility`.
    pub id: String,
    /// "Screen Recording".
    pub title: String,
    /// The pane to open.
    #[serde(default)]
    pub settings_url: Option<String>,
    /// What to turn on there.
    #[serde(default)]
    pub instructions: Option<String>,
    /// Already granted.
    #[serde(default)]
    pub granted: bool,
}

/// The host's state as the "This machine" page reads it (the cua SDK's
/// `HostStatus`, flattened).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostState {
    /// Set up for access.
    pub configured: bool,
    /// `relay` or `direct`.
    #[serde(default)]
    pub mode: Option<String>,
    /// Relay URL.
    #[serde(default)]
    pub relay_url: Option<String>,
    /// Direct URL.
    #[serde(default)]
    pub direct_url: Option<String>,
    /// Display name.
    #[serde(default)]
    pub name: Option<String>,
    /// Accepting clients.
    pub sharing: bool,
    /// Service installed.
    pub service_installed: bool,
    /// Service running.
    pub service_running: bool,
    /// `launchd`, `systemd`, `windows-task`, `process`.
    pub service_kind: String,
    /// Online at the relay.
    #[serde(default)]
    pub online: Option<bool>,
    /// Connected now.
    #[serde(default)]
    pub clients: Vec<HostClient>,
    /// Panes to grant.
    #[serde(default)]
    pub permissions: Vec<HostPermissionInput>,
    /// Needs attention.
    #[serde(default)]
    pub error: Option<String>,
    /// Who reached this machine recently, newest first.
    #[serde(default)]
    pub recent_access: Vec<HostAccess>,
    /// Set when the access log does not verify (edited or truncated).
    #[serde(default)]
    pub access_log_error: Option<String>,
    /// This machine's own desktop is a Space.
    #[serde(default = "yes")]
    pub share_desktop: bool,
    /// This machine creates Spaces for your other devices.
    #[serde(default)]
    pub provide_spaces: bool,
    /// Provided Spaces at once (0: no limit).
    #[serde(default)]
    pub max_spaces: u32,
    /// macOS VMs at once (at most two; 0 off a Mac).
    #[serde(default)]
    pub max_macos_vms: u32,
    /// The Spaces this machine provides now.
    #[serde(default)]
    pub provided_spaces: Vec<HostProvidedSpace>,
    /// Remote creates, deletes and refusals, and settings changes, newest
    /// first.
    #[serde(default)]
    pub spaces_audit: Vec<HostSpacesAudit>,
    /// Set when the Spaces audit does not verify.
    #[serde(default)]
    pub spaces_audit_error: Option<String>,
}

fn yes() -> bool {
    true
}

/// A Space this machine provides to one of your devices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProvidedSpace {
    /// The relay machine it is reached as.
    pub relay_machine: String,
    /// The Space on this machine (`local:<name>`).
    #[serde(default)]
    pub local_space: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// The image.
    #[serde(default)]
    pub image: String,
    /// `linux`, `macos`.
    #[serde(default)]
    pub os: String,
    /// `container` or `vm`.
    #[serde(default)]
    pub kind: String,
    /// Who created it.
    #[serde(default)]
    pub created_by: String,
    /// Epoch ms.
    #[serde(default)]
    pub created_at_ms: i64,
}

/// One line of the Spaces audit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSpacesAudit {
    /// Epoch ms.
    pub at_ms: i64,
    /// `create`, `delete`, `refused`, `failed`, `config`.
    pub action: String,
    /// Who (an account, or `local`).
    pub who: String,
    /// The relay machine or Space.
    #[serde(default)]
    pub space: String,
    /// Detail.
    #[serde(default)]
    pub detail: String,
}

impl HostState {
    /// What the sidebar row's summary reads.
    pub fn summary_input(&self) -> HostSummaryInput {
        HostSummaryInput {
            configured: self.configured,
            error: self.error.clone(),
            service_running: self.service_running,
            sharing: self.sharing,
            clients: self.clients.len() as u32,
            mode: self.mode.clone(),
            online: self.online,
        }
    }
}

/// What a "This machine" button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostActionId {
    /// Open the host setup form.
    SetUp,
    /// Cut clients and refuse new ones.
    StopSharing,
    /// Accept clients again.
    ResumeSharing,
    /// Unregister and uninstall the service.
    Remove,
    /// Share this machine's own desktop.
    ShareDesktop,
    /// Stop sharing this machine's own desktop (Spaces keep running).
    HideDesktop,
    /// Create Spaces for your other devices.
    ProvideSpaces,
    /// Stop creating Spaces for your other devices.
    StopProvidingSpaces,
}

/// A settings change a host action asks for (`None` keeps a value).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSettingChange {
    /// Share this desktop.
    #[serde(default)]
    pub share_desktop: Option<bool>,
    /// Provide Spaces.
    #[serde(default)]
    pub provide_spaces: Option<bool>,
}

/// The settings change a toggle action runs (`None` for the other
/// actions). Both shells pass it to the host's `configure`.
pub fn setting_change(id: HostActionId) -> Option<HostSettingChange> {
    let (share_desktop, provide_spaces) = match id {
        HostActionId::ShareDesktop => (Some(true), None),
        HostActionId::HideDesktop => (Some(false), None),
        HostActionId::ProvideSpaces => (None, Some(true)),
        HostActionId::StopProvidingSpaces => (None, Some(false)),
        _ => return None,
    };
    Some(HostSettingChange {
        share_desktop,
        provide_spaces,
    })
}

/// One of the two settings, drawn as a switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostToggle {
    /// `desktop` or `spaces`.
    pub id: String,
    /// Label.
    pub label: String,
    /// One line under it.
    pub help: String,
    /// On now.
    pub on: bool,
    /// Can be flipped (the last setting on cannot be turned off; Spaces
    /// need the relay).
    pub enabled: bool,
    /// What flipping it runs.
    pub action: HostActionId,
}

/// Asked before a button runs (both shells show it as a native alert).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostConfirm {
    /// The question.
    pub title: String,
    /// What happens.
    pub message: String,
    /// The confirming button.
    pub confirm_label: String,
    /// The other button.
    pub cancel_label: String,
}

/// A "This machine" button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAction {
    /// What it does.
    pub id: HostActionId,
    /// Label.
    pub label: String,
    /// Drawn as destructive.
    pub destructive: bool,
    /// Ask this first; the action runs only when the user confirms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<HostConfirm>,
}

/// The confirmation before removing this machine's host setup.
pub fn remove_confirm() -> HostConfirm {
    HostConfirm {
        title: "Remove host setup?".into(),
        message: "Everyone connected is disconnected, the host service is \
                  uninstalled and this machine leaves your machines on the \
                  relay. Setting it up again is needed to share it."
            .into(),
        confirm_label: "Remove".into(),
        cancel_label: "Cancel".into(),
    }
}

/// A row under "Recent access".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostAccessRow {
    /// "Ada · Files".
    pub text: String,
    /// Epoch ms (each shell shows it relative, natively).
    pub at_ms: i64,
}

/// What an access used, in words.
pub fn access_what(what: &str) -> String {
    match what {
        "ProcessService" => "Terminal and processes",
        "FilesystemService" => "Files",
        "TeleportService" => "Teleport",
        "TunnelService" => "Network tunnel",
        "DriverService" | "MCP" => "Automation tools",
        "SystemService" => "Status",
        "ComputerService"
        | "DesktopService"
        | "MediaService"
        | "WindowsService"
        | "AccessibilityService"
        | "PresenceService" => "Screen and input",
        other => other,
    }
    .to_string()
}

/// Who, in words: the token holder is named by the credential (a name it
/// sends is only its claim).
pub fn access_who(via: &str, who: &str) -> String {
    if via == "token" {
        if let Some(claimed) = who
            .strip_prefix("token (claims ")
            .and_then(|w| w.strip_suffix(')'))
        {
            return format!("{claimed} (access token)");
        }
        return "Access token".into();
    }
    who.to_string()
}

/// "Recent access" rows, newest first.
pub fn access_rows(recent: &[HostAccess]) -> Vec<HostAccessRow> {
    recent
        .iter()
        .map(|a| HostAccessRow {
            text: format!(
                "{} \u{b7} {}",
                access_who(&a.via, &a.who),
                access_what(&a.what)
            ),
            at_ms: a.at_ms,
        })
        .collect()
}

/// A permission row: the pane's title and "Open Settings".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRow {
    /// Id.
    pub id: String,
    /// "Screen Recording".
    pub title: String,
    /// Tooltip (what to turn on).
    pub help: String,
    /// The pane (only a `x-apple.systempreferences:` URL is opened).
    pub settings_url: Option<String>,
}

/// The "This machine" page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPanelView {
    /// "This machine".
    pub title: String,
    /// One line: how it is shared, or that it is not.
    pub summary: String,
    /// Set up for access.
    pub configured: bool,
    /// Name, Access, Service.
    pub facts: Vec<crate::spaces::sidebar::Fact>,
    /// "Connected now" (configured only).
    pub clients_title: Option<String>,
    /// One line per client.
    pub clients: Vec<String>,
    /// "Nobody" when nobody is connected.
    pub clients_empty: Option<String>,
    /// "Recent access" (configured, when the log has any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent_title: Option<String>,
    /// Who reached this machine recently, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<HostAccessRow>,
    /// Set when the access log does not verify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_warning: Option<String>,
    /// The two settings (configured only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub toggles: Vec<HostToggle>,
    /// The limits, one line ("Up to 4 Spaces \u{b7} 2 macOS VMs"), while
    /// providing Spaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<String>,
    /// "Spaces for your devices", while providing Spaces.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provided_title: Option<String>,
    /// One row per Space this machine provides.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provided: Vec<HostAccessRow>,
    /// "None yet" when it provides none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provided_empty: Option<String>,
    /// "Spaces activity" (when the audit has any).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_title: Option<String>,
    /// Remote creates, deletes and refusals, newest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activity: Vec<HostAccessRow>,
    /// Set when the Spaces audit does not verify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_warning: Option<String>,
    /// Heading over the permission rows, when any is left to grant.
    pub permissions_title: Option<String>,
    /// Panes still to grant.
    pub permissions: Vec<PermissionRow>,
    /// The permission rows' button.
    pub open_settings_label: String,
    /// Buttons, primary first.
    pub actions: Vec<HostAction>,
    /// Before setup: what this machine is and why set it up.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    /// Before setup: the ways to set it up, shown inline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub setup_choices: Vec<HostSetupChoice>,
}

/// A way to set this machine up, shown before setup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSetupChoice {
    /// The form's profile: `desktop` or `spare`.
    pub id: String,
    /// Label.
    pub label: String,
    /// The button that opens the form with this choice.
    pub button_label: String,
}

/// The intro over the setup choices.
pub const HOST_INTRO: &str = "This is the computer you are using now. Set it \
up for access to reach it and its Spaces from your other devices and your \
agents, over the Cua relay with no port forwarding.";

/// The setup choices (the form's "Use for" options).
pub fn setup_choices() -> Vec<HostSetupChoice> {
    vec![
        HostSetupChoice {
            id: "desktop".into(),
            label: "Share this desktop".into(),
            button_label: "Set Up\u{2026}".into(),
        },
        HostSetupChoice {
            id: "spare".into(),
            label: "Use as a spare machine for Spaces".into(),
            button_label: "Set Up\u{2026}".into(),
        },
    ]
}

/// Panes still to grant, as rows.
pub fn permission_rows(perms: &[HostPermissionInput]) -> Vec<PermissionRow> {
    perms
        .iter()
        .filter(|p| !p.granted)
        .map(|p| PermissionRow {
            id: p.id.clone(),
            title: p.title.clone(),
            help: p.instructions.clone().unwrap_or_default(),
            settings_url: p.settings_url.clone().filter(|u| !u.is_empty()),
        })
        .collect()
}

/// The heading over permission rows.
pub const PERMISSIONS_TITLE: &str = "Grant in System Settings";
/// Their button.
pub const OPEN_SETTINGS_LABEL: &str = "Open Settings";

/// The "This machine" page for `state` (`None` while it loads).
pub fn panel(state: Option<&HostState>) -> HostPanelView {
    use crate::spaces::sidebar::Fact;
    let mut v = HostPanelView {
        title: "This machine".into(),
        summary: "Checking\u{2026}".into(),
        configured: false,
        facts: vec![],
        clients_title: None,
        clients: vec![],
        clients_empty: None,
        recent_title: None,
        recent: vec![],
        access_warning: None,
        toggles: vec![],
        limits: None,
        provided_title: None,
        provided: vec![],
        provided_empty: None,
        activity_title: None,
        activity: vec![],
        activity_warning: None,
        permissions_title: None,
        permissions: vec![],
        open_settings_label: OPEN_SETTINGS_LABEL.into(),
        actions: vec![],
        intro: None,
        setup_choices: vec![],
    };
    let Some(s) = state else {
        return v;
    };
    if !s.configured {
        v.summary = "Other devices can\u{2019}t reach this machine yet.".into();
        v.intro = Some(HOST_INTRO.into());
        v.setup_choices = setup_choices();
        v.actions = vec![HostAction {
            id: HostActionId::SetUp,
            label: "Set up for access".into(),
            destructive: false,
            confirm: None,
        }];
        return v;
    }
    v.configured = true;
    v.summary = host_summary(Some(&s.summary_input()));
    let access = if s.mode.as_deref() == Some("direct") {
        format!(
            "Direct at {}",
            s.direct_url
                .as_deref()
                .filter(|u| !u.is_empty())
                .unwrap_or("this machine\u{2019}s address")
        )
    } else {
        match s.relay_url.as_deref().filter(|u| !u.is_empty()) {
            Some(u) => format!("Relay {u}"),
            None => "Relay".into(),
        }
    };
    let service = if s.service_running {
        "running"
    } else if s.service_installed {
        "stopped"
    } else {
        "not installed"
    };
    v.facts = vec![
        Fact {
            label: "Name".into(),
            value: s
                .name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| "This machine".into()),
            copy: None,
            help: None,
            warning: None,
        },
        Fact {
            label: "Access".into(),
            value: access,
            copy: None,
            help: None,
            warning: None,
        },
        Fact {
            label: "Service".into(),
            value: format!("{} \u{b7} {service}", s.service_kind),
            copy: None,
            help: None,
            warning: None,
        },
    ];
    v.clients_title = Some("Connected now".into());
    v.clients = s
        .clients
        .iter()
        .map(|c| {
            let who = client_label(&c.id, c.email.as_deref(), c.name.as_deref());
            match c.streams {
                Some(n) if n > 0 => {
                    format!("{who} \u{b7} {n} stream{}", if n == 1 { "" } else { "s" })
                }
                _ => who,
            }
        })
        .collect();
    v.clients_empty = s.clients.is_empty().then(|| "Nobody".into());
    v.recent = access_rows(&s.recent_access);
    v.recent_title = (!v.recent.is_empty()).then(|| "Recent access".into());
    v.access_warning = s
        .access_log_error
        .as_ref()
        .map(|_| "The access log was changed outside Cua; it may be incomplete.".into());
    let relay = s.mode.as_deref() != Some("direct");
    v.toggles = vec![
        HostToggle {
            id: "desktop".into(),
            label: "Share this desktop".into(),
            help: "Your devices can see and control this screen.".into(),
            on: s.share_desktop,
            enabled: relay && (!s.share_desktop || s.provide_spaces),
            action: if s.share_desktop {
                HostActionId::HideDesktop
            } else {
                HostActionId::ShareDesktop
            },
        },
        HostToggle {
            id: "spaces".into(),
            label: "Provide Spaces".into(),
            help: if relay {
                "Your devices can create Spaces here.".into()
            } else {
                "Needs the relay.".into()
            },
            on: s.provide_spaces,
            enabled: relay && (!s.provide_spaces || s.share_desktop),
            action: if s.provide_spaces {
                HostActionId::StopProvidingSpaces
            } else {
                HostActionId::ProvideSpaces
            },
        },
    ];
    if s.provide_spaces {
        v.limits = Some(limits_line(s.max_spaces, s.max_macos_vms));
        v.provided_title = Some("Spaces for your devices".into());
        v.provided = provided_rows(&s.provided_spaces);
        v.provided_empty = s.provided_spaces.is_empty().then(|| "None yet".into());
    }
    v.activity = activity_rows(&s.spaces_audit);
    v.activity_title = (!v.activity.is_empty()).then(|| "Spaces activity".into());
    v.activity_warning = s
        .spaces_audit_error
        .as_ref()
        .map(|_| "The Spaces activity log does not verify; it may have been changed.".into());
    // Screen permissions matter only while the desktop is shared.
    v.permissions = if s.share_desktop {
        permission_rows(&s.permissions)
    } else {
        vec![]
    };
    v.permissions_title = (!v.permissions.is_empty()).then(|| PERMISSIONS_TITLE.into());
    v.actions = vec![
        if s.sharing {
            // One click, never a confirmation: stopping access must be
            // the easy direction.
            HostAction {
                id: HostActionId::StopSharing,
                label: "Stop sharing".into(),
                destructive: true,
                confirm: None,
            }
        } else {
            HostAction {
                id: HostActionId::ResumeSharing,
                label: "Resume sharing".into(),
                destructive: false,
                confirm: None,
            }
        },
        HostAction {
            id: HostActionId::Remove,
            label: "Remove host setup".into(),
            destructive: true,
            confirm: Some(remove_confirm()),
        },
    ];
    v
}

/// "Up to 4 Spaces · 2 macOS VMs (Apple's license allows two per Mac)".
pub fn limits_line(max_spaces: u32, max_macos_vms: u32) -> String {
    let spaces = if max_spaces == 0 {
        "No Space limit".to_string()
    } else {
        format!(
            "Up to {max_spaces} Space{}",
            if max_spaces == 1 { "" } else { "s" }
        )
    };
    if max_macos_vms == 0 {
        return spaces;
    }
    format!(
        "{spaces} \u{b7} {max_macos_vms} macOS VM{} (Apple\u{2019}s license allows two per Mac)",
        if max_macos_vms == 1 { "" } else { "s" }
    )
}

/// One row per provided Space: "space-1 · macOS · Ada".
pub fn provided_rows(spaces: &[HostProvidedSpace]) -> Vec<HostAccessRow> {
    spaces
        .iter()
        .map(|p| {
            let name = if p.name.is_empty() {
                &p.relay_machine
            } else {
                &p.name
            };
            let os = match p.os.as_str() {
                "macos" => "macOS",
                "linux" => "Linux",
                "windows" => "Windows",
                _ => "",
            };
            let mut text = name.to_string();
            if !os.is_empty() {
                text.push_str(&format!(" \u{b7} {os}"));
            }
            if !p.created_by.is_empty() {
                text.push_str(&format!(" \u{b7} {}", p.created_by));
            }
            HostAccessRow {
                text,
                at_ms: p.created_at_ms,
            }
        })
        .collect()
}

/// An audit action, in words.
pub fn activity_action(action: &str) -> &str {
    match action {
        "create" => "Created",
        "delete" => "Deleted",
        "refused" => "Refused",
        "failed" => "Failed",
        "config" => "Settings changed",
        other => other,
    }
}

/// "Spaces activity" rows, newest first: "Created space-1 · Ada".
pub fn activity_rows(audit: &[HostSpacesAudit]) -> Vec<HostAccessRow> {
    audit
        .iter()
        .map(|a| {
            let mut text = activity_action(&a.action).to_string();
            if a.action == "config" {
                let on = |key: &str| a.detail.contains(&format!("{key}=on"));
                if a.detail.contains("share_desktop=") {
                    text.push_str(&format!(
                        " \u{b7} desktop {}, Spaces {}",
                        if on("share_desktop") { "on" } else { "off" },
                        if on("provide_spaces") { "on" } else { "off" },
                    ));
                }
            } else {
                if !a.space.is_empty() {
                    text.push_str(&format!(" {}", a.space));
                }
                text.push_str(&format!(" \u{b7} {}", a.who));
                if (a.action == "refused" || a.action == "failed")
                    && let Some(first) = a.detail.lines().next().filter(|d| !d.is_empty())
                {
                    text.push_str(&format!(" \u{b7} {first}"));
                }
            }
            HostAccessRow {
                text,
                at_ms: a.at_ms,
            }
        })
        .collect()
}

/// The host setup form's state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HostFormState {
    /// Display name.
    pub name: String,
    /// "Also allow" (relay).
    pub allow: String,
    /// Advanced is open.
    pub advanced: bool,
    /// Direct instead of the relay (Advanced).
    pub direct: bool,
    /// Direct listen address.
    pub listen: String,
    /// Relay URL (Advanced).
    pub relay_url: String,
    /// Setup is running.
    pub busy: bool,
    /// The last failure.
    pub error: Option<String>,
    /// A spare machine: do not share its desktop, provide Spaces (relay
    /// only).
    pub spare: bool,
}

impl Default for HostFormState {
    fn default() -> Self {
        Self {
            name: String::new(),
            allow: String::new(),
            advanced: false,
            direct: false,
            listen: DEFAULT_LISTEN.into(),
            relay_url: DEFAULT_RELAY_URL.into(),
            busy: false,
            error: None,
            spare: false,
        }
    }
}

/// An input to the form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum HostFormAction {
    /// Name typed.
    SetName {
        /// Text.
        name: String,
    },
    /// "Also allow" typed.
    SetAllow {
        /// Text.
        allow: String,
    },
    /// Advanced opened or closed.
    ToggleAdvanced,
    /// "Direct connection" switched.
    SetDirect {
        /// On.
        on: bool,
    },
    /// Listen address typed.
    SetListen {
        /// Text.
        listen: String,
    },
    /// Relay URL typed.
    SetRelayUrl {
        /// Text.
        url: String,
    },
    /// What the machine is for: `desktop` or `spare`.
    SetProfile {
        /// `desktop` or `spare`.
        profile: String,
    },
    /// "Set up for access" pressed (ignored unless it can submit).
    Submit,
    /// Setup failed.
    Failed {
        /// Why.
        error: String,
    },
}

/// A field (or the Direct switch), in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostFormField {
    /// `name`, `allow`, `direct`, `listen`, `relay`.
    pub id: String,
    /// Label.
    pub label: String,
    /// Placeholder (text fields).
    pub placeholder: Option<String>,
    /// Text.
    pub value: String,
    /// The switch (`direct` only).
    pub toggle: bool,
    /// On (`direct` only).
    pub on: bool,
    /// Drawn as invalid.
    pub invalid: bool,
    /// Under Advanced.
    pub advanced: bool,
    /// A choice of one (`profile` only): `value` is the chosen id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<HostFormChoice>,
}

/// One option of a choice field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostFormChoice {
    /// `desktop` or `spare`.
    pub id: String,
    /// Label.
    pub label: String,
}

/// The host setup form as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostFormView {
    /// "Set up this machine".
    pub title: String,
    /// One line: what setting up does.
    pub lede: String,
    /// Visible fields in order (Advanced ones only while it is open).
    pub fields: Vec<HostFormField>,
    /// "Advanced".
    pub advanced_label: String,
    /// Advanced is open.
    pub advanced_open: bool,
    /// "Back".
    pub back_label: String,
    /// "Set up for access" or "Setting up…".
    pub submit_label: String,
    /// The submit button is enabled.
    pub can_submit: bool,
    /// Setup is running.
    pub busy: bool,
    /// The last failure, one line.
    pub error: Option<String>,
    /// What host setup receives (validated), when it can submit.
    pub request: Option<HostSetupRequest>,
}

/// The first form.
pub fn form_initial() -> HostFormState {
    HostFormState::default()
}

fn form_request(s: &HostFormState) -> Result<HostSetupRequest, String> {
    let allow = parse_allow_list(&s.allow);
    validate_setup(HostSetupRequest {
        mode: if s.direct { "direct" } else { "relay" }.into(),
        relay_url: (!s.direct).then(|| s.relay_url.clone()),
        direct: s.direct.then(|| s.listen.clone()),
        name: Some(s.name.clone()),
        allow: (!s.direct && !allow.is_empty()).then_some(allow),
        profile: Some(
            if s.spare && !s.direct {
                "spare"
            } else {
                "desktop"
            }
            .into(),
        ),
        share_desktop: None,
        provide_spaces: None,
    })
    .and_then(|r| {
        if r.mode == "relay" && r.relay_url.is_none() {
            Err("relay URL needed".into())
        } else {
            Ok(r)
        }
    })
}

/// Advances the form.
pub fn form_reduce(s: &HostFormState, a: &HostFormAction) -> HostFormState {
    let mut n = s.clone();
    match a {
        HostFormAction::SetName { name } => n.name = name.clone(),
        HostFormAction::SetAllow { allow } => n.allow = allow.clone(),
        HostFormAction::ToggleAdvanced => n.advanced = !s.advanced,
        HostFormAction::SetDirect { on } => n.direct = *on,
        HostFormAction::SetListen { listen } => n.listen = listen.clone(),
        HostFormAction::SetRelayUrl { url } => n.relay_url = url.clone(),
        HostFormAction::SetProfile { profile } => n.spare = profile == "spare",
        HostFormAction::Submit => {
            if !s.busy && form_request(s).is_ok() {
                n.busy = true;
                n.error = None;
            }
        }
        HostFormAction::Failed { error } => {
            n.busy = false;
            n.error = Some(
                error
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("Host setup failed")
                    .trim()
                    .to_string(),
            );
        }
    }
    n
}

/// The form as drawn. `identity` is the signed-in account (relay mode joins
/// as it).
pub fn form_view(s: &HostFormState, identity: Option<&str>) -> HostFormView {
    let text =
        |id: &str, label: &str, placeholder: &str, value: &str, invalid: bool, advanced: bool| {
            HostFormField {
                id: id.into(),
                label: label.into(),
                placeholder: (!placeholder.is_empty()).then(|| placeholder.to_string()),
                value: value.into(),
                toggle: false,
                on: false,
                invalid,
                advanced,
                choices: vec![],
            }
        };
    let mut fields = vec![text(
        "name",
        "Name",
        "This computer\u{2019}s name",
        &s.name,
        false,
        false,
    )];
    if !s.direct {
        fields.push(HostFormField {
            id: "profile".into(),
            label: "Use for".into(),
            placeholder: None,
            value: if s.spare { "spare" } else { "desktop" }.into(),
            toggle: false,
            on: false,
            invalid: false,
            advanced: false,
            choices: vec![
                HostFormChoice {
                    id: "desktop".into(),
                    label: "Its desktop".into(),
                },
                HostFormChoice {
                    id: "spare".into(),
                    label: "A spare machine for Spaces".into(),
                },
            ],
        });
        fields.push(text(
            "allow",
            "Also allow",
            "Emails, optional",
            &s.allow,
            false,
            false,
        ));
    }
    if s.advanced {
        fields.push(HostFormField {
            id: "direct".into(),
            label: "Direct connection".into(),
            placeholder: None,
            value: String::new(),
            toggle: true,
            on: s.direct,
            invalid: false,
            advanced: true,
            choices: vec![],
        });
        if s.direct {
            let bad = !s.listen.trim().is_empty() && form_request(s).is_err();
            fields.push(text(
                "listen",
                "Listen on",
                DEFAULT_LISTEN,
                &s.listen,
                bad,
                true,
            ));
        } else {
            let bad = !s.relay_url.trim().is_empty() && form_request(s).is_err();
            fields.push(text(
                "relay",
                "Relay",
                DEFAULT_RELAY_URL,
                &s.relay_url,
                bad,
                true,
            ));
        }
    }
    let lede = if s.direct {
        "Other machines connect to this address with an access token.".to_string()
    } else {
        let what = if s.spare {
            " Runs Spaces for your devices; its desktop stays private."
        } else {
            ""
        };
        match identity.filter(|i| !i.is_empty()) {
            Some(id) => format!("Joins the Cua relay as {id}. No port forwarding.{what}"),
            None => format!("Joins the Cua relay. No port forwarding.{what}"),
        }
    };
    let request = form_request(s).ok();
    HostFormView {
        title: "Set up this machine".into(),
        lede,
        fields,
        advanced_label: "Advanced".into(),
        advanced_open: s.advanced,
        back_label: "Back".into(),
        submit_label: if s.busy {
            "Setting up\u{2026}"
        } else {
            "Set up for access"
        }
        .into(),
        can_submit: request.is_some() && !s.busy,
        busy: s.busy,
        error: s.error.clone(),
        request,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_needs_a_port() {
        assert!(looks_like_listen("10.0.0.5:3211"));
        assert!(looks_like_listen("[::1]:3211"));
        assert!(looks_like_listen("host.local:80"));
        assert!(!looks_like_listen("10.0.0.5"));
        assert!(!looks_like_listen("10.0.0.5:0"));
        assert!(!looks_like_listen("10.0.0.5:70000"));
    }

    #[test]
    fn summary_reads_the_sharing_state() {
        assert_eq!(host_summary(None), "Set up for access");
        let s = HostSummaryInput {
            configured: true,
            service_running: true,
            sharing: true,
            clients: 2,
            ..Default::default()
        };
        assert_eq!(host_summary(Some(&s)), "Sharing \u{b7} 2 connected");
    }

    #[test]
    fn the_dot_is_green_only_while_sharing_for_access() {
        let sharing = HostSummaryInput {
            configured: true,
            service_running: true,
            sharing: true,
            ..Default::default()
        };
        assert_eq!(
            this_machine_status(None),
            SpaceStatus::Suspended,
            "not set up"
        );
        assert_eq!(this_machine_status(Some(&sharing)), SpaceStatus::Local);
        for off in [
            HostSummaryInput {
                configured: false,
                ..sharing.clone()
            },
            HostSummaryInput {
                service_running: false,
                ..sharing.clone()
            },
            HostSummaryInput {
                sharing: false,
                ..sharing.clone()
            },
            HostSummaryInput {
                online: Some(false),
                ..sharing.clone()
            },
        ] {
            assert_eq!(
                this_machine_status(Some(&off)),
                SpaceStatus::Suspended,
                "{off:?}"
            );
        }
        let broken = HostSummaryInput {
            error: Some("relay refused".into()),
            ..sharing.clone()
        };
        assert_eq!(this_machine_status(Some(&broken)), SpaceStatus::Approval);
        // Still labelled and drawn as usable, whatever the dot says.
        let row = this_machine_space(None, 0, SpaceOs::Macos);
        assert_eq!(crate::spaces::sidebar::status_text(&row), "This Mac");
    }

    #[test]
    fn setup_requests_are_validated() {
        let relay = validate_setup(HostSetupRequest {
            mode: "Relay".into(),
            relay_url: Some(" https://relay.example/ ".into()),
            direct: Some("ignored".into()),
            name: Some("  ".into()),
            allow: Some(vec![" a@x.com ".into(), "".into()]),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(relay.mode, "relay");
        assert_eq!(relay.relay_url.as_deref(), Some("https://relay.example"));
        assert_eq!(relay.direct, None);
        assert_eq!(relay.name, None);
        assert_eq!(relay.allow, Some(vec!["a@x.com".to_string()]));
        assert!(
            validate_setup(HostSetupRequest {
                mode: "relay".into(),
                relay_url: Some("ftp://x".into()),
                ..Default::default()
            })
            .is_err()
        );
        let direct = validate_setup(HostSetupRequest {
            mode: "direct".into(),
            direct: Some("0.0.0.0:3211".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(direct.direct.as_deref(), Some("0.0.0.0:3211"));
        assert!(
            validate_setup(HostSetupRequest {
                mode: "direct".into(),
                direct: Some("somewhere".into()),
                ..Default::default()
            })
            .is_err()
        );
        assert!(
            validate_setup(HostSetupRequest {
                mode: "teleport".into(),
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn this_machine_comes_first_once() {
        let spaces = with_this_machine(&[], None, 5, SpaceOs::Macos);
        assert_eq!(spaces.len(), 1);
        let again = with_this_machine(&spaces, None, 6, SpaceOs::Macos);
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].id, THIS_MACHINE_ID);
        assert_eq!(again[0].detail, "Set up for access");
    }

    #[test]
    fn the_panel_offers_setup_then_sharing_controls() {
        assert!(panel(None).actions.is_empty());
        let off = panel(Some(&HostState::default()));
        assert_eq!(off.actions[0].id, HostActionId::SetUp);
        // Before setup the page explains itself and shows the form's two
        // choices inline.
        assert!(
            off.intro
                .as_deref()
                .is_some_and(|t| !t.contains('\u{2014}'))
        );
        assert_eq!(
            off.setup_choices
                .iter()
                .map(|c| c.id.as_str())
                .collect::<Vec<_>>(),
            ["desktop", "spare"]
        );
        let on = panel(Some(&HostState {
            configured: true,
            sharing: true,
            service_running: true,
            service_installed: true,
            service_kind: "launchd".into(),
            permissions: vec![HostPermissionInput {
                id: "accessibility".into(),
                title: "Accessibility".into(),
                granted: true,
                ..Default::default()
            }],
            ..Default::default()
        }));
        assert_eq!(on.actions[0].id, HostActionId::StopSharing);
        assert!(on.actions[0].destructive);
        assert!(on.permissions.is_empty() && on.permissions_title.is_none());
        assert_eq!(on.clients_empty.as_deref(), Some("Nobody"));
    }

    #[test]
    fn stopping_is_one_click_and_removing_asks_first() {
        let on = panel(Some(&HostState {
            configured: true,
            sharing: true,
            service_running: true,
            ..Default::default()
        }));
        assert_eq!(on.actions[0].id, HostActionId::StopSharing);
        assert!(on.actions[0].confirm.is_none(), "stop is one click");
        let remove = &on.actions[1];
        assert_eq!(remove.id, HostActionId::Remove);
        assert!(remove.destructive);
        assert_eq!(remove.confirm.as_ref().unwrap().confirm_label, "Remove");
    }

    #[test]
    fn the_panel_lists_recent_access_and_flags_an_altered_log() {
        let state = HostState {
            configured: true,
            sharing: true,
            service_running: true,
            recent_access: vec![
                HostAccess {
                    at_ms: 3,
                    via: "relay".into(),
                    who: "Ada (acct-1)".into(),
                    what: "FilesystemService".into(),
                },
                HostAccess {
                    at_ms: 2,
                    via: "token".into(),
                    who: "token (claims Mallory)".into(),
                    what: "MCP".into(),
                },
                HostAccess {
                    at_ms: 1,
                    via: "token".into(),
                    who: "token".into(),
                    what: "ProcessService".into(),
                },
            ],
            ..Default::default()
        };
        let v = panel(Some(&state));
        assert_eq!(v.recent_title.as_deref(), Some("Recent access"));
        let texts: Vec<&str> = v.recent.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Ada (acct-1) \u{b7} Files",
                "Mallory (access token) \u{b7} Automation tools",
                "Access token \u{b7} Terminal and processes",
            ]
        );
        assert_eq!(v.recent[0].at_ms, 3);
        assert!(v.access_warning.is_none());
        let altered = panel(Some(&HostState {
            access_log_error: Some("line 2: altered".into()),
            ..state
        }));
        assert!(altered.access_warning.is_some());
        // Nothing to show: no heading.
        assert!(
            panel(Some(&HostState {
                configured: true,
                ..Default::default()
            }))
            .recent_title
            .is_none()
        );
    }

    #[test]
    fn connected_now_reads_back_the_entry_the_core_built() {
        let input = HostSummaryInput {
            configured: true,
            service_running: true,
            sharing: true,
            clients: 2,
            ..Default::default()
        };
        let me = this_machine_space(Some(&input), 0, SpaceOs::Macos);
        assert_eq!(connected_now(&me), 2);
        let idle = this_machine_space(
            Some(&HostSummaryInput {
                clients: 0,
                ..input.clone()
            }),
            0,
            SpaceOs::Macos,
        );
        assert_eq!(connected_now(&idle), 0);
        // Only the "This machine" entry counts.
        let mut other = me.clone();
        other.id = "local:x".into();
        assert_eq!(connected_now(&other), 0);
    }

    #[test]
    fn the_form_submits_only_a_valid_request() {
        let mut s = form_initial();
        let v = form_view(&s, None);
        assert!(v.can_submit);
        assert_eq!(
            v.fields.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["name", "profile", "allow"]
        );
        s = form_reduce(&s, &HostFormAction::ToggleAdvanced);
        s = form_reduce(&s, &HostFormAction::SetDirect { on: true });
        s = form_reduce(
            &s,
            &HostFormAction::SetListen {
                listen: "nowhere".into(),
            },
        );
        assert!(!form_view(&s, None).can_submit);
        let busy = form_reduce(&s, &HostFormAction::Submit);
        assert!(!busy.busy, "an invalid form does not submit");
        s = form_reduce(
            &s,
            &HostFormAction::SetListen {
                listen: "127.0.0.1:4000".into(),
            },
        );
        s = form_reduce(&s, &HostFormAction::Submit);
        assert!(s.busy);
        let v = form_view(&s, None);
        assert_eq!(v.submit_label, "Setting up\u{2026}");
        assert_eq!(v.request.unwrap().direct.as_deref(), Some("127.0.0.1:4000"));
        s = form_reduce(
            &s,
            &HostFormAction::Failed {
                error: "\nbad thing\nmore".into(),
            },
        );
        assert_eq!(s.error.as_deref(), Some("bad thing"));
        assert!(!s.busy);
    }

    fn host(desktop: bool, provide: bool) -> HostState {
        HostState {
            configured: true,
            sharing: true,
            service_running: true,
            mode: Some("relay".into()),
            share_desktop: desktop,
            provide_spaces: provide,
            max_spaces: 4,
            max_macos_vms: 2,
            permissions: vec![HostPermissionInput {
                id: "screen-recording".into(),
                title: "Screen Recording".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn the_two_settings_are_switches_and_one_stays_on() {
        let v = panel(Some(&host(true, false)));
        let desktop = &v.toggles[0];
        assert_eq!(
            (desktop.label.as_str(), desktop.on),
            ("Share this desktop", true)
        );
        assert!(!desktop.enabled, "the only setting on cannot be turned off");
        assert_eq!(v.toggles[1].action, HostActionId::ProvideSpaces);
        assert!(v.toggles[1].enabled);
        assert!(v.limits.is_none() && v.provided_title.is_none());
        assert_eq!(v.permissions.len(), 1);

        // A spare machine: no desktop, so no screen permissions to grant.
        let v = panel(Some(&host(false, true)));
        assert_eq!(v.toggles[0].action, HostActionId::ShareDesktop);
        assert!(v.toggles[0].enabled);
        assert!(!v.toggles[1].enabled);
        assert!(v.permissions.is_empty() && v.permissions_title.is_none());
        assert_eq!(
            v.limits.as_deref(),
            Some("Up to 4 Spaces \u{b7} 2 macOS VMs (Apple\u{2019}s license allows two per Mac)")
        );
        assert_eq!(v.provided_empty.as_deref(), Some("None yet"));

        // Both on: either can go off.
        let v = panel(Some(&host(true, true)));
        assert!(v.toggles.iter().all(|t| t.enabled));
        // Direct mode cannot provide Spaces or hide the desktop.
        let direct = panel(Some(&HostState {
            mode: Some("direct".into()),
            ..host(true, false)
        }));
        assert!(direct.toggles.iter().all(|t| !t.enabled));
        assert_eq!(direct.toggles[1].help, "Needs the relay.");

        assert_eq!(
            setting_change(HostActionId::HideDesktop),
            Some(HostSettingChange {
                share_desktop: Some(false),
                provide_spaces: None
            })
        );
        assert_eq!(
            setting_change(HostActionId::ProvideSpaces)
                .unwrap()
                .provide_spaces,
            Some(true)
        );
        assert!(setting_change(HostActionId::Remove).is_none());
    }

    #[test]
    fn provided_spaces_and_their_activity_are_listed() {
        let mut s = host(false, true);
        s.provided_spaces = vec![HostProvidedSpace {
            relay_machine: "space-1".into(),
            name: "mac-1".into(),
            os: "macos".into(),
            created_by: "Ada".into(),
            created_at_ms: 7,
            ..Default::default()
        }];
        s.spaces_audit = vec![
            HostSpacesAudit {
                at_ms: 9,
                action: "refused".into(),
                who: "Bob".into(),
                space: "space-2".into(),
                detail: "already runs 2 macOS VMs".into(),
            },
            HostSpacesAudit {
                at_ms: 8,
                action: "create".into(),
                who: "Ada".into(),
                space: "space-1".into(),
                detail: "local:mac-1".into(),
            },
            HostSpacesAudit {
                at_ms: 1,
                action: "config".into(),
                who: "local".into(),
                space: "Mac mini".into(),
                detail: "share_desktop=off provide_spaces=on".into(),
            },
        ];
        let v = panel(Some(&s));
        assert_eq!(v.provided_title.as_deref(), Some("Spaces for your devices"));
        assert_eq!(v.provided[0].text, "mac-1 \u{b7} macOS \u{b7} Ada");
        assert!(v.provided_empty.is_none());
        assert_eq!(v.activity_title.as_deref(), Some("Spaces activity"));
        let texts: Vec<&str> = v.activity.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "Refused space-2 \u{b7} Bob \u{b7} already runs 2 macOS VMs",
                "Created space-1 \u{b7} Ada",
                "Settings changed \u{b7} desktop off, Spaces on",
            ]
        );
        assert!(v.activity_warning.is_none());
        s.spaces_audit_error = Some("line 2: altered".into());
        assert!(panel(Some(&s)).activity_warning.is_some());
    }

    #[test]
    fn the_form_offers_a_spare_machine_on_the_relay_only() {
        let s = form_reduce(
            &form_initial(),
            &HostFormAction::SetProfile {
                profile: "spare".into(),
            },
        );
        let v = form_view(&s, Some("ada@example.com"));
        let profile = v.fields.iter().find(|f| f.id == "profile").unwrap();
        assert_eq!(profile.value, "spare");
        assert_eq!(
            profile
                .choices
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            ["Its desktop", "A spare machine for Spaces"]
        );
        assert!(v.lede.contains("its desktop stays private"), "{}", v.lede);
        let request = v.request.unwrap();
        assert_eq!(request.profile.as_deref(), Some("spare"));
        assert_eq!(request.settings(), (false, true));
        // Direct has no profile: it always shares the desktop.
        let mut d = form_reduce(&s, &HostFormAction::ToggleAdvanced);
        d = form_reduce(&d, &HostFormAction::SetDirect { on: true });
        let v = form_view(&d, None);
        assert!(v.fields.iter().all(|f| f.id != "profile"));
        assert_eq!(v.request.unwrap().settings(), (true, false));
    }

    #[test]
    fn setup_settings_are_validated() {
        let spare = validate_setup(HostSetupRequest {
            mode: "relay".into(),
            profile: Some(" Spare ".into()),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(spare.profile.as_deref(), Some("spare"));
        for bad in [
            HostSetupRequest {
                mode: "relay".into(),
                profile: Some("laptop".into()),
                ..Default::default()
            },
            HostSetupRequest {
                mode: "relay".into(),
                share_desktop: Some(false),
                ..Default::default()
            },
            HostSetupRequest {
                mode: "direct".into(),
                direct: Some("0.0.0.0:3211".into()),
                provide_spaces: Some(true),
                ..Default::default()
            },
            HostSetupRequest {
                mode: "direct".into(),
                direct: Some("0.0.0.0:3211".into()),
                profile: Some("spare".into()),
                ..Default::default()
            },
        ] {
            assert!(validate_setup(bad.clone()).is_err(), "{bad:?}");
        }
    }
}
