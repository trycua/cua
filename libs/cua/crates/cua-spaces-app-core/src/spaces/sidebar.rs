// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The main window: the sidebar (This machine, then one section per
//! location), search, the selection fallback, and the selected Space's
//! detail (facts, whether it streams, what Delete means).

use crate::host::THIS_MACHINE_ID;
use crate::model::{
    PowerControl, Space, SpaceProvider, SpaceStatus, arch_label, emulation_warning, normalize_arch,
};
use serde::{Deserialize, Serialize};

/// Section titles, in order.
pub const SECTION_ORDER: [&str; 4] = ["Cua Cloud", "This Mac", "Connected", "My machines"];

fn section_of(p: Option<SpaceProvider>) -> &'static str {
    match p.unwrap_or(SpaceProvider::Cloud) {
        SpaceProvider::Cloud => "Cua Cloud",
        SpaceProvider::Local => "This Mac",
        SpaceProvider::Direct => "Connected",
        SpaceProvider::Relay => "My machines",
    }
}

/// The status word for a Space row and its detail.
pub fn status_text(space: &Space) -> String {
    if space.progress.as_ref().is_some_and(|p| p.error.is_some()) {
        return "Failed".into();
    }
    if space.progress.as_ref().is_some_and(|p| p.cancelling) {
        return "Cancelling\u{2026}".into();
    }
    // This machine is always usable here; its dot says whether it is
    // shared for access, its detail line says how.
    if space.id == THIS_MACHINE_ID {
        return "This Mac".into();
    }
    if space.status != SpaceStatus::Deleting
        && let Some(power) = &space.power
    {
        if let Some(on) = power.turning_on {
            return power_verb(power.control, on).into();
        }
        if power.off && space.status == SpaceStatus::Suspended {
            return super::off_label(power.control).into();
        }
    }
    match space.status {
        SpaceStatus::Running | SpaceStatus::Local => {
            if space.id == THIS_MACHINE_ID {
                "This Mac".into()
            } else {
                "Running".into()
            }
        }
        s => s.label().into(),
    }
}

/// The power button's SF Symbol.
pub const POWER_SYMBOL: &str = "power";

/// "Suspending…", "Resuming…", "Turning off…", "Turning on…".
pub fn power_verb(control: PowerControl, on: bool) -> &'static str {
    match (control, on) {
        (PowerControl::Suspend, false) => "Suspending\u{2026}",
        (PowerControl::Suspend, true) => "Resuming\u{2026}",
        (PowerControl::Stop, false) => "Turning off\u{2026}",
        (PowerControl::Stop, true) => "Turning on\u{2026}",
    }
}

/// The power button next to Delete, on every Space row and in the Space's
/// toolbar, for a Space whose provider can turn it off and on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerButton {
    /// SF Symbol ([`POWER_SYMBOL`]).
    pub symbol: String,
    /// Tooltip and accessibility label: "Suspend", "Resume", "Turn off" or
    /// "Turn on"; while it runs, "Suspending…" and the like.
    pub help: String,
    /// A press turns it on (else off): the shell's SDK call.
    pub turn_on: bool,
    /// It can be pressed (not while an action runs).
    pub enabled: bool,
    /// An action runs: a spinner shows in its place.
    pub busy: bool,
}

/// The power button of `space`, when it has one: not for this machine, a
/// Space being created or deleted, or one whose provider cannot.
pub fn power_button(space: &Space) -> Option<PowerButton> {
    let power = space.power.as_ref()?;
    if space.id == THIS_MACHINE_ID
        || space.progress.is_some()
        || space.status == SpaceStatus::Deleting
    {
        return None;
    }
    let busy = power.turning_on.is_some();
    let turn_on = power.turning_on.unwrap_or(power.off);
    let help = if busy {
        power_verb(power.control, turn_on)
    } else {
        match (power.control, turn_on) {
            (PowerControl::Suspend, false) => "Suspend",
            (PowerControl::Suspend, true) => "Resume",
            (PowerControl::Stop, false) => "Turn off",
            (PowerControl::Stop, true) => "Turn on",
        }
    };
    Some(PowerButton {
        symbol: POWER_SYMBOL.into(),
        help: help.into(),
        turn_on,
        enabled: !busy,
        busy,
    })
}

/// Why the Space's last power action failed, shown inline.
pub fn power_error(space: &Space) -> Option<String> {
    power_button(space)?;
    space.power.as_ref()?.error.clone()
}

/// Where a Space runs, in words.
pub fn location_text(space: &Space) -> &'static str {
    if space.id == THIS_MACHINE_ID {
        return "This Mac";
    }
    match space.provider.unwrap_or(SpaceProvider::Cloud) {
        SpaceProvider::Cloud => "Cua Cloud",
        SpaceProvider::Local => "This Mac",
        SpaceProvider::Direct => "By address",
        SpaceProvider::Relay => "Relay",
    }
}

/// Spaces cua did not create (by address, relay) are only removed from the
/// list; deleting leaves the machine alone.
///
/// A Space one of your machines provides (it names its `host`) was created
/// by cua there, so deleting it deletes it on that machine. A Space in your
/// cloud is deleted through the records of the device that created it.
pub fn added_by_address(space: &Space) -> bool {
    match space.provider {
        Some(SpaceProvider::Direct) => true,
        Some(SpaceProvider::Relay) => {
            space.host.as_deref().is_none_or(str::is_empty) && !in_your_cloud(space)
        }
        _ => false,
    }
}

/// A Space in your cloud (AWS, Google Cloud, Modal).
pub fn in_your_cloud(space: &Space) -> bool {
    space.cloud.as_deref().is_some_and(|c| !c.is_empty())
}

/// One sidebar row: one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidebarRow {
    /// Space id.
    pub id: String,
    /// Name.
    pub name: String,
    /// The Space's OS icon id ([`crate::notch::os_icon`], the same mark the
    /// notch tiles use), before the name.
    pub os_icon: String,
    /// Status (the dot).
    pub status: SpaceStatus,
    /// Status word (the dot's accessibility label).
    pub status_text: String,
    /// Tooltip.
    pub detail: String,
    /// Drawn at reduced opacity.
    pub dim: bool,
    /// Selected.
    pub selected: bool,
    /// While it is being created: overall progress in thousandths (a ring).
    pub progress: Option<u32>,
    /// Short text after the name: the percentage while it is being
    /// created, the error when the create failed.
    pub trailing: Option<String>,
    /// A Space one of your machines provides, drawn nested under that
    /// machine's row (the row before it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nested: bool,
    /// The power button (next to Delete), for a Space whose provider can
    /// turn it off and on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power: Option<PowerButton>,
    /// Where a Space in your cloud runs ("AWS · us-west-2"), secondary
    /// text after the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
}

/// A titled group of rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidebarSection {
    /// "Cua Cloud", "This Mac", "Connected", "My machines".
    pub title: String,
    /// Rows.
    pub rows: Vec<SidebarRow>,
}

/// The sidebar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidebarView {
    /// The "This machine" row, when it matches the search.
    pub this_machine: Option<SidebarRow>,
    /// Location sections, in [`SECTION_ORDER`].
    pub sections: Vec<SidebarSection>,
    /// The effective selection (falls back to the first Space).
    pub selected_id: Option<String>,
    /// "No matches" / "No Spaces yet" when nothing is listed.
    pub empty_text: Option<String>,
}

fn row(space: &Space, selected: Option<&str>) -> SidebarRow {
    SidebarRow {
        id: space.id.clone(),
        name: space.name.clone(),
        os_icon: crate::notch::os_icon(space.os, space.os_name.as_deref()).into(),
        status: space.status,
        status_text: status_text(space),
        detail: space.detail.clone(),
        dim: space.id != THIS_MACHINE_ID && !space.status.is_live(),
        selected: selected == Some(space.id.as_str()),
        progress: space
            .progress
            .as_ref()
            .filter(|p| p.error.is_none())
            .map(|p| p.permille),
        // Why a create failed, else why turning it off or on failed.
        trailing: space
            .progress
            .as_ref()
            .map(|p| match &p.error {
                Some(e) => e.clone(),
                None => percent(p.permille),
            })
            .or_else(|| power_error(space)),
        nested: false,
        power: power_button(space),
        place: space
            .cloud_place
            .clone()
            .filter(|_| in_your_cloud(space) && space.progress.is_none()),
    }
}

/// Rows of one section with every Space a machine provides right after
/// that machine, nested; one whose machine is not listed stays a plain row
/// (its detail names the machine).
fn nest_under_hosts(spaces: Vec<&Space>, selected: Option<&str>) -> Vec<SidebarRow> {
    let host_of = |s: &Space| s.host.clone().filter(|h| !h.is_empty());
    let listed = |h: &str| {
        spaces
            .iter()
            .any(|m| m.id.strip_prefix("relay:") == Some(h) && host_of(m).is_none())
    };
    let mut out = Vec::with_capacity(spaces.len());
    for s in spaces
        .iter()
        .filter(|s| host_of(s).is_none_or(|h| !listed(&h)))
    {
        out.push(row(s, selected));
        if let Some(machine) = s.id.strip_prefix("relay:").filter(|_| host_of(s).is_none()) {
            for child in spaces
                .iter()
                .filter(|c| host_of(c).as_deref() == Some(machine))
            {
                let mut r = row(child, selected);
                r.nested = true;
                out.push(r);
            }
        }
    }
    out
}

/// Thousandths as a whole percentage ("42%").
pub fn percent(permille: u32) -> String {
    format!("{}%", permille.min(1000) / 10)
}

/// The selection the window shows: the chosen id if it still exists, else
/// the first Space that is not this machine, else the first.
pub fn effective_selection<'a>(spaces: &'a [Space], selected_id: &str) -> Option<&'a Space> {
    spaces
        .iter()
        .find(|s| s.id == selected_id)
        .or_else(|| spaces.iter().find(|s| s.id != THIS_MACHINE_ID))
        .or_else(|| spaces.first())
}

/// Builds the sidebar for `spaces` filtered by `query`.
pub fn sidebar(spaces: &[Space], query: &str, selected_id: &str) -> SidebarView {
    let needle = query.trim().to_lowercase();
    let visible: Vec<&Space> = spaces
        .iter()
        .filter(|s| {
            needle.is_empty()
                || [&s.name, &s.detail, &s.id]
                    .iter()
                    .any(|f| f.to_lowercase().contains(&needle))
        })
        .collect();
    let selected = effective_selection(spaces, selected_id).map(|s| s.id.clone());
    let sel = selected.as_deref();
    let this_machine = visible
        .iter()
        .find(|s| s.id == THIS_MACHINE_ID)
        .map(|s| row(s, sel));
    let sections: Vec<SidebarSection> = SECTION_ORDER
        .iter()
        .filter_map(|title| {
            let rows = nest_under_hosts(
                visible
                    .iter()
                    .copied()
                    .filter(|s| s.id != THIS_MACHINE_ID && section_of(s.provider) == *title)
                    .collect(),
                sel,
            );
            (!rows.is_empty()).then(|| SidebarSection {
                title: (*title).to_string(),
                rows,
            })
        })
        .collect();
    let empty_text = sections.is_empty().then(|| {
        if needle.is_empty() {
            "No Spaces yet".to_string()
        } else {
            "No matches".to_string()
        }
    });
    SidebarView {
        this_machine,
        sections,
        selected_id: selected,
        empty_text,
    }
}

/// One label and value of the detail list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fact {
    /// Label.
    pub label: String,
    /// Value.
    pub value: String,
    /// A copy button after the value (the Space's identifier and image).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy: Option<FactCopy>,
    /// The tooltip: the full value (and an image's digest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// A warning symbol after the value, with its tooltip: a local Space
    /// emulating another architecture.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<FactWarning>,
}

/// A fact's warning: an icon after the value and its tooltip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FactWarning {
    /// SF Symbol ([`WARNING_SYMBOL`]).
    pub symbol: String,
    /// Tooltip and accessibility label.
    pub help: String,
}

pub use crate::model::WARNING_SYMBOL;

/// A fact's copy button: an icon button that puts `text` on the pasteboard,
/// then shows `done_symbol` and `done_help` for `confirm_ms`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FactCopy {
    /// What is copied (the full value).
    pub text: String,
    /// SF Symbol (`doc.on.doc`).
    pub symbol: String,
    /// Tooltip and accessibility label ("Copy").
    pub help: String,
    /// The symbol while confirming (`checkmark`).
    pub done_symbol: String,
    /// The tooltip while confirming ("Copied").
    pub done_help: String,
    /// How long the confirmation shows, in milliseconds.
    pub confirm_ms: u32,
}

/// The copy button for `text`.
pub fn fact_copy(text: &str) -> FactCopy {
    FactCopy {
        text: text.into(),
        symbol: "doc.on.doc".into(),
        help: "Copy".into(),
        done_symbol: "checkmark".into(),
        done_help: "Copied".into(),
        confirm_ms: 1_500,
    }
}

/// The selected Space's detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceDetail {
    /// Space id.
    pub id: String,
    /// Title.
    pub title: String,
    /// Status, Image, System, Memory, Storage, Identifier (each when known).
    pub facts: Vec<Fact>,
    /// The host machine itself.
    pub is_host: bool,
    /// The Stream / Agents / Teleport sections show.
    pub show_sections: bool,
    /// Streaming is possible (toolbar PiP and Stream).
    pub can_stream: bool,
    /// Placeholder over the preview until a frame arrives.
    pub preview_text: String,
    /// While it is being created: overall progress in thousandths (the
    /// preview's ring).
    pub progress: Option<u32>,
    /// Under the bar while it downloads: "4.2 of 22.1 GB · 85 MB/s · about
    /// 4 min".
    #[serde(default)]
    pub progress_text: Option<String>,
    /// "Remove" (added by address) or "Delete Space".
    pub delete_label: String,
    /// Removing only forgets it.
    pub remove_only: bool,
    /// Toolbar buttons, in order (none for this machine).
    pub actions: Vec<DetailAction>,
    /// What Delete asks first.
    pub confirm: DeleteConfirm,
    /// The sections under the facts, in order (Stream, Agents, Teleport).
    pub sections: Vec<String>,
    /// A cloud Space refused for want of credit: one line and "Add
    /// credit" (the website billing page).
    #[serde(default)]
    pub credit_notice: Option<crate::billing::CreditNotice>,
    /// Why turning it off or on failed, shown inline under the preview.
    #[serde(default)]
    pub power_error: Option<String>,
    /// Signed in, but this device is not enrolled: a machine reached
    /// through the relay is listed with its Connect greyed out under this
    /// line and its one action ([`detail_for`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<crate::devices::MachineAccessNotice>,
    /// One of your machines that does not share its desktop: the line
    /// shown in place of its desktop, Stream, Agents and Teleport
    /// ([`desktop_note`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub desktop_note: Option<String>,
    /// With [`Self::desktop_note`], when the machine provides Spaces:
    /// "New Space on <name>…", which opens New Space on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_space: Option<NewSpaceOn>,
}

/// "New Space on <machine>…": New Space with "Run on" set to that machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSpaceOn {
    /// The button's label.
    pub label: String,
    /// The "Run on" entry it picks (`host:<machine id>`; the wizard's
    /// `ChoosePlacement`).
    pub on: String,
}

/// spacesd's feature for a machine that provides Spaces.
pub const HOST_SPACES_FEATURE: &str = "host_spaces";

/// One of your machines itself, reached through the relay: not a Space it
/// provides and not a Space in your cloud.
pub fn is_your_machine(space: &Space) -> bool {
    space.id.starts_with("relay:")
        && space.host.as_deref().is_none_or(str::is_empty)
        && !in_your_cloud(space)
}

/// Whether one of your machines shares its own desktop, when it said so:
/// it answered with its features, and a machine that does not share its
/// desktop reports the desktop's features unsupported to you (cua-spacesd
/// with `share_desktop` off). `None` when it is not one of your machines
/// or has not answered (an older cua-spacesd always reports them, so it
/// counts as sharing, as before).
pub fn shares_desktop(space: &Space) -> Option<bool> {
    let sdk = space.sdk.as_ref()?;
    if !is_your_machine(space) || !sdk.reachable || sdk.features.is_empty() {
        return None;
    }
    Some(super::has_feature(space, super::feature::DESKTOP_STREAM))
}

/// The line in place of a machine's desktop when it does not share it:
/// that you can still create Spaces on it, or that it shares neither.
pub fn desktop_note(space: &Space) -> Option<String> {
    if shares_desktop(space)? {
        return None;
    }
    let name = &space.name;
    Some(if super::has_feature(space, HOST_SPACES_FEATURE) {
        format!("{name} isn\u{2019}t sharing its desktop. You can still create Spaces on it.")
    } else {
        format!("{name} isn\u{2019}t sharing its desktop or Spaces.")
    })
}

/// The roster without this machine's own relay entry
/// (`relay:<this_machine_id>`, listed once it shares its desktop or
/// provides Spaces): this machine has its own place ("This machine"), not
/// one under "My machines". Matched by this install's relay machine id,
/// never by name, so another machine of the same name stays; the Spaces it
/// provides stay too.
pub fn without_this_relay_machine(spaces: &[Space], this_machine_id: Option<&str>) -> Vec<Space> {
    let Some(own) = this_machine_id
        .filter(|id| !id.is_empty())
        .map(|id| format!("relay:{id}"))
    else {
        return spaces.to_vec();
    };
    spaces.iter().filter(|s| s.id != own).cloned().collect()
}

/// The Spaces one of your machines provides, as sidebar rows (its detail
/// lists them under New Space).
pub fn hosted_rows(spaces: &[Space], machine_space_id: &str, selected_id: &str) -> Vec<SidebarRow> {
    let Some(machine) = machine_space_id.strip_prefix("relay:") else {
        return vec![];
    };
    spaces
        .iter()
        .filter(|s| s.host.as_deref() == Some(machine))
        .map(|s| row(s, Some(selected_id)))
        .collect()
}

/// What a toolbar button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DetailActionId {
    /// "Teleport an app…" into the Space.
    Teleport,
    /// The desktop in a picture-in-picture panel.
    Pip,
    /// "Share": let another cua.ai account watch or edit the Space.
    Share,
    /// Turn it off or on (the power button, next to Delete).
    Power,
    /// Delete (or remove from the list).
    Delete,
    /// The desktop in its own window.
    Open,
    /// Stop a create that is still running and remove what it made.
    Cancel,
}

/// A toolbar button.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailAction {
    /// What it does.
    pub id: DetailActionId,
    /// Label.
    pub label: String,
    /// SF Symbol (icon buttons).
    pub symbol: Option<String>,
    /// Tooltip.
    pub help: String,
    /// Enabled.
    pub enabled: bool,
    /// Drawn as destructive.
    pub destructive: bool,
    /// The prominent one.
    pub primary: bool,
    /// Its action runs: a spinner in place of the icon.
    #[serde(default)]
    pub busy: bool,
}

/// The question Delete asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConfirm {
    /// "Delete Brave Otter?"
    pub title: String,
    /// What it means, one line.
    pub message: String,
    /// The destructive button.
    pub confirm_label: String,
    /// The destructive button can be pressed (a Space in your cloud that
    /// another device created cannot be deleted from here).
    pub confirm_enabled: bool,
    /// Why not, beside the disabled button.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// For a Space in your cloud: "Remove from List", which forgets it
    /// and keeps it running there (`remove_space`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remove_label: Option<String>,
    /// "Keep" (or "Cancel" beside Remove from List).
    pub cancel_label: String,
}

/// The one teleport icon: the toolbar's "Teleport an app", the drop well
/// and every other teleport affordance.
pub const TELEPORT_SYMBOL: &str = "arrow.down.circle";
/// The same icon while something is dragged over a drop target.
pub const TELEPORT_SYMBOL_ACTIVE: &str = "arrow.down.circle.fill";

/// The words of a Space's sections: Stream, Agents, Teleport. The same in
/// every Space; states the shells show while a section loads, is empty or
/// failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailCopy {
    /// "Looking for this Space's windows…"
    pub stream_loading: String,
    /// "No open windows in this Space yet."
    pub stream_empty: String,
    /// When the window list could not be read.
    pub stream_failed: String,
    /// When a filter hides every window.
    pub stream_no_match: String,
    /// "Looking for this Space's agents…"
    pub agents_loading: String,
    /// "No agents have been started in this Space."
    pub agents_empty: String,
    /// When the runs could not be read (not the same as none).
    pub agents_failed: String,
    /// When a filter hides every run.
    pub agents_no_match: String,
    /// The drop well's caption.
    pub drop_caption: String,
    /// "Send file…"
    pub send_file: String,
    /// "Teleport an app…"
    pub teleport_app: String,
    /// The teleport icon ([`TELEPORT_SYMBOL`]).
    pub teleport_symbol: String,
    /// The teleport icon while a drag is over a drop target.
    pub teleport_symbol_active: String,
}

/// The sections' words.
pub fn detail_copy() -> DetailCopy {
    DetailCopy {
        stream_loading: "Looking for this Space\u{2019}s windows\u{2026}".into(),
        stream_empty: "No open windows in this Space yet.".into(),
        stream_failed: "No windows: the Space\u{2019}s window host is not up.".into(),
        stream_no_match: "No windows match your filter.".into(),
        agents_loading: "Looking for this Space\u{2019}s agents\u{2026}".into(),
        agents_empty: "No agents have been started in this Space.".into(),
        agents_failed: "Could not read this Space\u{2019}s agents.".into(),
        agents_no_match: "No agents match your filter.".into(),
        drop_caption: "Drop a file or window".into(),
        send_file: "Send file\u{2026}".into(),
        teleport_app: "Teleport an app\u{2026}".into(),
        teleport_symbol: TELEPORT_SYMBOL.into(),
        teleport_symbol_active: TELEPORT_SYMBOL_ACTIVE.into(),
    }
}

/// What Delete asks first. A Space in your cloud offers both: Delete
/// Permanently (what Cua created there is deleted, through the records of
/// the device that created it) and Remove from List (it keeps running).
fn delete_confirm(space: &Space, remove_only: bool, delete_label: &str) -> DeleteConfirm {
    let name = &space.name;
    if in_your_cloud(space) && !remove_only {
        let place = space
            .cloud_place
            .as_deref()
            .or(space.cloud.as_deref())
            .unwrap_or_default();
        let elsewhere = space.cloud_delete.as_deref() == Some("elsewhere");
        let message = if elsewhere {
            format!(
                "It runs in {place} and was created on another device. Remove from List keeps it running there."
            )
        } else {
            format!(
                "Delete Permanently deletes it and everything Cua created for it in {place}. Remove from List keeps it running there."
            )
        };
        return DeleteConfirm {
            title: format!("Delete {name}?"),
            message,
            confirm_label: "Delete Permanently".into(),
            confirm_enabled: !elsewhere,
            disabled_reason: elsewhere.then(|| {
                "Created on another device: delete it there, or remove it from this list.".into()
            }),
            remove_label: Some("Remove from List".into()),
            cancel_label: "Cancel".into(),
        };
    }
    DeleteConfirm {
        title: format!("{} {name}?", if remove_only { "Remove" } else { "Delete" },),
        message: if remove_only {
            "It is only removed from the list."
        } else {
            "Its sandbox is deleted."
        }
        .into(),
        confirm_label: delete_label.into(),
        confirm_enabled: true,
        disabled_reason: None,
        remove_label: None,
        cancel_label: "Keep".into(),
    }
}

/// The banner when Delete fails.
pub fn delete_failed_text(name: &str, error: &str) -> String {
    format!(
        "Could not delete {name}: {}",
        crate::errors::plain_error(error)
    )
}

/// Memory and storage use of a Space (the SDK's `Space.usage`), for its
/// Memory and Storage facts. A size shows only when it is the guest's own
/// limit (`*_limited`: a VM, a container's cgroup memory limit).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceUsage {
    /// Memory in use, bytes.
    pub memory_used: u64,
    /// Memory size, bytes.
    pub memory_total: u64,
    /// `memory_total` is the guest's limit.
    pub memory_limited: bool,
    /// Storage in use, bytes.
    pub disk_used: u64,
    /// Storage size, bytes.
    pub disk_total: u64,
    /// `disk_total` is the guest's own disk.
    pub disk_limited: bool,
}

/// How often a shell refreshes [`SpaceUsage`] while a detail is visible.
pub const USAGE_REFRESH_MS: u64 = 10_000;

/// A size in binary units, labelled the way macOS labels memory: "512 MB",
/// "2.1 GB", "4 GB".
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["MB", "GB", "TB", "PB"];
    let mut v = bytes as f64 / (1024.0 * 1024.0);
    if v < 1.0 {
        return format!("{} KB", (bytes as f64 / 1024.0).round() as u64);
    }
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    let text = if v >= 100.0 {
        format!("{}", v.round() as u64)
    } else {
        let t = crate::to_fixed(v, 1);
        t.strip_suffix(".0").map(str::to_string).unwrap_or(t)
    };
    format!("{text} {}", UNITS[i])
}

/// "2.1 GB / 4 GB".
pub fn usage_text(used: u64, total: u64) -> String {
    format!("{} / {}", format_size(used.min(total)), format_size(total))
}

/// The OS in words: the full string the Space reported ("Ubuntu 24.04.3
/// LTS"), else its product name, else the family ("Linux").
pub fn system_text(space: &Space) -> String {
    [&space.os_pretty_name, &space.os_name]
        .into_iter()
        .flatten()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| space.os.label().into())
}

fn fact(label: &str, value: String) -> Fact {
    Fact {
        label: label.into(),
        value,
        copy: None,
        help: None,
        warning: None,
    }
}

/// The Architecture fact: "ARM" or "x64", with a warning when a local
/// Space runs another architecture than this Mac's (`host_arch`): it is
/// emulated, and slow.
pub fn arch_fact(space: &Space, host_arch: Option<&str>) -> Option<Fact> {
    let arch = space.arch.as_deref().and_then(normalize_arch)?;
    let local = space.provider == Some(SpaceProvider::Local);
    Some(Fact {
        warning: emulation_warning(local, host_arch, Some(arch)).map(|help| FactWarning {
            symbol: WARNING_SYMBOL.into(),
            help,
        }),
        ..fact("Architecture", arch_label(arch).unwrap_or(arch).into())
    })
}

/// Status, Image, Identifier, System (the full OS string), Kind,
/// Architecture, Memory, Storage: each only when known.
fn detail_facts(
    space: &Space,
    status: String,
    usage: Option<&SpaceUsage>,
    host_arch: Option<&str>,
) -> Vec<Fact> {
    let mut facts = vec![fact("Status", status)];
    if let Some(place) = space
        .cloud_place
        .as_deref()
        .filter(|_| in_your_cloud(space))
    {
        facts.push(fact("Location", place.to_string()));
    }
    if let Some(image) = space.image.as_deref().filter(|i| !i.is_empty()) {
        facts.push(Fact {
            help: Some(
                match space.image_digest.as_deref().filter(|d| !d.is_empty()) {
                    Some(digest) => format!("{image}\n{digest}"),
                    None => image.to_string(),
                },
            ),
            copy: Some(fact_copy(image)),
            ..fact("Image", image.to_string())
        });
    }
    // Directly below the Image. A Space being created has no identifier yet.
    if space.progress.is_none() {
        facts.push(Fact {
            copy: Some(fact_copy(&space.id)),
            ..fact("Identifier", space.id.clone())
        });
    }
    let system = system_text(space);
    facts.push(Fact {
        help: Some(system.clone()),
        ..fact("System", system)
    });
    if let Some(kind) = space.kind {
        facts.push(fact("Kind", kind.label().into()));
    }
    facts.extend(arch_fact(space, host_arch));
    if let Some(u) = usage {
        if u.memory_limited && u.memory_total > 0 {
            facts.push(fact("Memory", usage_text(u.memory_used, u.memory_total)));
        }
        if u.disk_limited && u.disk_total > 0 {
            facts.push(fact("Storage", usage_text(u.disk_used, u.disk_total)));
        }
    }
    facts
}

/// The detail of `space`.
pub fn detail(space: &Space) -> SpaceDetail {
    detail_live(space, None, None)
}

/// The detail of `space` with its current memory and storage use.
/// `host_arch` is this machine's CPU architecture (`arm64`, `x86_64`, ...):
/// a local Space of another one shows a warning on its Architecture.
pub fn detail_live(
    space: &Space,
    usage: Option<&SpaceUsage>,
    host_arch: Option<&str>,
) -> SpaceDetail {
    let reachable = space.sdk.as_ref().is_some_and(|s| s.reachable);
    let is_host = space.id == THIS_MACHINE_ID;
    let creating = space.progress.as_ref();
    let failed = creating.and_then(|p| p.error.clone());
    let deleting = space.status == SpaceStatus::Deleting;
    // Off, or turning on or off: the power word, not the probe's error.
    let powered = space.power.as_ref().is_some_and(|p| {
        p.turning_on.is_some() || (p.off && space.status == SpaceStatus::Suspended)
    });
    let status = match (&space.sdk, creating) {
        _ if deleting || powered => status_text(space),
        (_, Some(p)) => match &p.error {
            Some(_) => "Failed".to_string(),
            None => format!("{} {}", p.label, percent(p.permille)),
        },
        (Some(sdk), None) if !sdk.reachable => sdk
            .error
            .clone()
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| "Unreachable".into()),
        _ => status_text(space),
    };
    let preview_text = match (creating, &failed) {
        _ if deleting => status_text(space),
        (_, Some(e)) => e.clone(),
        (Some(p), None) => p.label.clone(),
        (None, None) if space.status == SpaceStatus::Provisioning => "Starting\u{2026}".into(),
        // Turning on or off, or off: said so, not "Not reachable".
        (None, None) if powered => status_text(space),
        (None, None) if reachable => "Loading the desktop\u{2026}".into(),
        (None, None) => "Not reachable".into(),
    };
    let remove_only = added_by_address(space) || super::creating::is_pending(&space.id);
    let can_stream = !is_host
        && creating.is_none()
        && space.sdk.as_ref().is_none_or(|s| s.reachable)
        && space.status != SpaceStatus::Provisioning
        && !deleting;
    let delete_label = if remove_only {
        "Remove"
    } else {
        "Delete Space"
    };
    let mut actions = Vec::new();
    if !is_host {
        let action = |id, label: &str, symbol: Option<&str>, help: &str, enabled| DetailAction {
            id,
            label: label.into(),
            symbol: symbol.map(str::to_string),
            help: help.into(),
            enabled,
            destructive: id == DetailActionId::Delete,
            primary: id == DetailActionId::Open,
            busy: false,
        };
        actions.push(action(
            DetailActionId::Teleport,
            "Teleport an app",
            Some(TELEPORT_SYMBOL),
            "Teleport an app into this Space",
            can_stream,
        ));
        actions.push(action(
            DetailActionId::Pip,
            "Picture in picture",
            Some("pip.enter"),
            "Picture in picture",
            can_stream,
        ));
        actions.push(action(
            DetailActionId::Share,
            "Share",
            Some("person.badge.plus"),
            "Share this Space",
            can_stream,
        ));
        // One still being created can be cancelled (it stops, and what it
        // made is removed).
        if let Some(p) = creating.filter(|p| p.error.is_none()) {
            actions.push(DetailAction {
                id: DetailActionId::Cancel,
                // The same width either way (the row and the preview say
                // Cancelling).
                label: "Cancel".into(),
                symbol: None,
                help: "Stop creating this Space and remove what it made".into(),
                enabled: p.cancellable,
                destructive: false,
                primary: false,
                busy: false,
            });
        }
        // The power button, next to Delete.
        if let Some(b) = power_button(space) {
            actions.push(DetailAction {
                id: DetailActionId::Power,
                label: b.help.clone(),
                symbol: Some(b.symbol),
                help: b.help,
                enabled: b.enabled,
                destructive: false,
                primary: false,
                busy: b.busy,
            });
        }
        // A failed create is removed from the list; one still starting has
        // nothing to delete yet; one being deleted is already going.
        if space.sdk.is_some() || failed.is_some() {
            actions.push(action(
                DetailActionId::Delete,
                delete_label,
                Some("trash"),
                if deleting {
                    "Deleting this Space\u{2026}"
                } else if remove_only {
                    "Remove from the list"
                } else if in_your_cloud(space) && creating.is_none() {
                    "Delete this Space or remove it from the list"
                } else {
                    "Delete this Space"
                },
                !deleting,
            ));
        }
        actions.push(action(
            DetailActionId::Open,
            "Open",
            None,
            "Open the desktop in its own window",
            can_stream,
        ));
    }
    let show_sections = space.sdk.is_some() && reachable && !deleting;
    SpaceDetail {
        id: space.id.clone(),
        title: space.name.clone(),
        facts: detail_facts(space, status, usage, host_arch),
        is_host,
        show_sections,
        can_stream,
        preview_text,
        progress: creating.filter(|p| p.error.is_none()).map(|p| p.permille),
        progress_text: creating
            .filter(|p| p.error.is_none())
            .and_then(|p| p.transfer.clone()),
        credit_notice: creating.and_then(|p| {
            p.credit_url
                .clone()
                .map(|url| crate::billing::CreditNotice {
                    text: crate::billing::OUT_OF_CREDIT.into(),
                    button: crate::billing::ADD_CREDIT.into(),
                    url,
                })
        }),
        delete_label: delete_label.into(),
        remove_only,
        power_error: power_error(space),
        access: None,
        desktop_note: None,
        new_space: None,
        actions,
        confirm: delete_confirm(space, remove_only, delete_label),
        sections: if show_sections {
            ["Stream", "Agents", "Teleport"]
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            vec![]
        },
    }
}

/// [`detail_live`] without what Settings, Experiments hides: the Share
/// button while Sharing is off (a Space already shared stays shared; its
/// people keep their access).
pub fn detail_with(
    space: &Space,
    usage: Option<&SpaceUsage>,
    host_arch: Option<&str>,
    experiments: &crate::experiments::Experiments,
) -> SpaceDetail {
    let mut d = detail_live(space, usage, host_arch);
    if !experiments.sharing {
        d.actions.retain(|a| a.id != DetailActionId::Share);
    }
    d
}

/// [`detail_with`] as this device sees it. `access` is why this device
/// cannot open the account's machines (signed in, not enrolled;
/// [`crate::devices::machine_access_notice`]): every Space reached through
/// the relay then shows its Connect greyed out under that line, says why
/// in its Status, and nothing that needs a connection is offered. One of
/// your machines that does not share its desktop shows [`desktop_note`] in
/// place of its desktop, Stream, Agents and Teleport, and New Space on it
/// when it provides Spaces.
pub fn detail_for(
    space: &Space,
    usage: Option<&SpaceUsage>,
    host_arch: Option<&str>,
    experiments: &crate::experiments::Experiments,
    access: Option<&crate::devices::MachineAccessNotice>,
) -> SpaceDetail {
    let mut d = detail_with(space, usage, host_arch, experiments);
    let connection = [
        DetailActionId::Teleport,
        DetailActionId::Pip,
        DetailActionId::Share,
        DetailActionId::Open,
    ];
    if let Some(access) = access.filter(|_| space.id.starts_with("relay:")) {
        d.can_stream = false;
        d.show_sections = false;
        d.sections.clear();
        d.preview_text = access.text.clone();
        if let Some(status) = d.facts.iter_mut().find(|f| f.label == "Status") {
            status.value = access.status.clone();
        }
        for a in d.actions.iter_mut().filter(|a| connection.contains(&a.id)) {
            a.enabled = false;
        }
        d.access = Some(access.clone());
        return d;
    }
    if let Some(note) = desktop_note(space) {
        d.can_stream = false;
        d.show_sections = false;
        d.sections.clear();
        d.preview_text = note.clone();
        d.actions.retain(|a| {
            !matches!(
                a.id,
                DetailActionId::Teleport | DetailActionId::Pip | DetailActionId::Open
            )
        });
        d.new_space = super::has_feature(space, HOST_SPACES_FEATURE).then(|| NewSpaceOn {
            label: format!("New Space on {}\u{2026}", space.name),
            on: format!("host:{}", space.id.trim_start_matches("relay:")),
        });
        d.desktop_note = Some(note);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SpaceRow;

    /// Sharing off (the default): no Share button; on: Share, between
    /// Picture in picture and Delete as before.
    #[test]
    fn the_sharing_experiment_decides_the_share_button() {
        use crate::experiments::Experiments;
        let s = super::super::row_to_space(&local_row("local:a", None, None, None), 0);
        let ids = |e: &Experiments| -> Vec<DetailActionId> {
            detail_with(&s, None, None, e)
                .actions
                .iter()
                .map(|a| a.id)
                .collect()
        };
        assert!(!ids(&Experiments::default()).contains(&DetailActionId::Share));
        let on = ids(&Experiments {
            sharing: true,
            ..Default::default()
        });
        assert_eq!(
            on.iter().position(|a| *a == DetailActionId::Share),
            Some(2),
            "{on:?}"
        );
        assert_eq!(
            detail_with(&s, None, None, &Experiments::all_on()),
            detail_live(&s, None, None)
        );
    }

    fn local_row(
        id: &str,
        image: Option<&str>,
        kind: Option<&str>,
        arch: Option<&str>,
    ) -> SpaceRow {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "name": "box",
            "provider": id.split(':').next().unwrap(),
            "os": "linux",
            "reachable": true,
            "image": image,
            "kind": kind,
            "arch": arch,
        }))
        .unwrap()
    }

    fn fact_of<'a>(d: &'a SpaceDetail, label: &str) -> Option<&'a Fact> {
        d.facts.iter().find(|f| f.label == label)
    }

    /// One of your machines on the relay, as the app's probe saw it.
    fn machine(features: &[&str], reachable: bool) -> Space {
        let row: SpaceRow = serde_json::from_value(serde_json::json!({
            "id": "relay:m1",
            "name": "Studio",
            "provider": "relay",
            "os": "macos",
            "reachable": reachable,
            "features": features,
        }))
        .unwrap();
        super::super::row_to_space(&row, 0)
    }

    fn ids(d: &SpaceDetail) -> Vec<DetailActionId> {
        d.actions.iter().map(|a| a.id).collect()
    }

    const SHARED: &[&str] = &["desktop_stream", "window_stream", "host_spaces"];

    /// Enrolled: nothing changes. Not enrolled, waiting or expired: the
    /// machine is listed, Connect is greyed out under the notice, Status
    /// says why and nothing that needs a connection is offered.
    #[test]
    fn a_machine_seen_from_a_device_that_is_not_enrolled() {
        use crate::devices::{EnrollmentKind, machine_access_notice};
        let x = crate::experiments::Experiments::default();
        let m = machine(SHARED, true);
        let enrolled = detail_for(&m, None, None, &x, None);
        assert_eq!(enrolled, detail_with(&m, None, None, &x));
        assert!(enrolled.can_stream && enrolled.access.is_none());
        for (kind, status) in [
            (EnrollmentKind::NeedsEnrollment, "Not enrolled"),
            (EnrollmentKind::Waiting, "Waiting for approval"),
            (EnrollmentKind::Due, "Re-verification due"),
        ] {
            let notice = machine_access_notice(kind, Some("K7QX-M2RP")).unwrap();
            // Unreachable (the relay refuses it): still listed, still explained.
            for reachable in [false, true] {
                let d = detail_for(&machine(&[], reachable), None, None, &x, Some(&notice));
                assert_eq!(d.access.as_ref(), Some(&notice));
                assert!(!d.can_stream && !d.show_sections && d.sections.is_empty());
                assert_eq!(fact_of(&d, "Status").unwrap().value, status);
                assert!(
                    d.actions
                        .iter()
                        .filter(|a| matches!(
                            a.id,
                            DetailActionId::Open | DetailActionId::Teleport | DetailActionId::Pip
                        ))
                        .all(|a| !a.enabled)
                );
                assert!(d.desktop_note.is_none());
            }
        }
        // A Space that is not reached through the relay is not affected.
        let local = super::super::row_to_space(&local_row("local:a", None, None, None), 0);
        let notice = machine_access_notice(EnrollmentKind::NeedsEnrollment, None).unwrap();
        assert!(
            detail_for(&local, None, None, &x, Some(&notice))
                .access
                .is_none()
        );
    }

    /// Desktop shared: Stream, Agents and Teleport as before. Not shared
    /// (Spaces still provided): the note, no desktop actions, New Space on
    /// it. Neither: the note alone. A machine that has not answered, or an
    /// older one that does not say, counts as sharing.
    #[test]
    fn a_machine_that_does_not_share_its_desktop() {
        let x = crate::experiments::Experiments::default();
        let shared = detail_for(&machine(SHARED, true), None, None, &x, None);
        assert_eq!(shared.sections, ["Stream", "Agents", "Teleport"]);
        assert!(shared.desktop_note.is_none() && shared.new_space.is_none());
        assert_eq!(shares_desktop(&machine(SHARED, true)), Some(true));

        let spaces_only = machine(&["host_spaces", "files"], true);
        assert_eq!(shares_desktop(&spaces_only), Some(false));
        let d = detail_for(&spaces_only, None, None, &x, None);
        assert_eq!(
            d.desktop_note.as_deref(),
            Some("Studio isn\u{2019}t sharing its desktop. You can still create Spaces on it.")
        );
        assert!(!d.can_stream && !d.show_sections && d.sections.is_empty());
        assert!(ids(&d).iter().all(|a| !matches!(
            a,
            DetailActionId::Teleport | DetailActionId::Pip | DetailActionId::Open
        )));
        assert_eq!(
            d.new_space,
            Some(NewSpaceOn {
                label: "New Space on Studio\u{2026}".into(),
                on: "host:m1".into()
            })
        );

        let neither = detail_for(&machine(&["files"], true), None, None, &x, None);
        assert_eq!(
            neither.desktop_note.as_deref(),
            Some("Studio isn\u{2019}t sharing its desktop or Spaces.")
        );
        assert!(neither.new_space.is_none() && neither.sections.is_empty());

        // Unknown: offline, or no features reported.
        assert_eq!(shares_desktop(&machine(&["host_spaces"], false)), None);
        assert_eq!(shares_desktop(&machine(&[], true)), None);
        assert!(
            detail_for(&machine(&[], true), None, None, &x, None)
                .desktop_note
                .is_none()
        );
        // Only your machines: a Space added by address without a desktop is
        // not "not sharing".
        let direct =
            super::super::row_to_space(&local_row("direct:10.0.0.5:3211", None, None, None), 0);
        assert_eq!(shares_desktop(&direct), None);
    }

    /// The user's Macs after updating to 0.7.0: cua.local
    /// (`relay:9e9d…`, "Mac.localdomain") shares, keeps its desktop
    /// private, provides Spaces with none running and runs the cua-spacesd
    /// of an older app (0.2.2); this Mac (`relay:b695…`) is not sharing.
    const OTHER_MAC: &str = "9e9d969d41649428f693b403f6f8f225";
    const THIS_MAC: &str = "b6953d14ff94b10fcbb38bdd13c47824";

    fn roster_row(
        id: &str,
        name: &str,
        version: &str,
        features: &[&str],
        reachable: bool,
        error: Option<&str>,
    ) -> Space {
        let row: SpaceRow = serde_json::from_value(serde_json::json!({
            "id": id,
            "name": name,
            "provider": id.split(':').next().unwrap(),
            "spacesdVersion": version,
            "features": features,
            "os": "macos",
            "reachable": reachable,
            "error": error,
        }))
        .unwrap();
        super::super::row_to_space(&row, 0)
    }

    /// One of your machines that is online and provides Spaces is listed
    /// under "My machines" whatever it shares and whichever cua-spacesd it
    /// runs: desktop off with Spaces on and none running (a current host
    /// says so in its features), an older host that reports every feature
    /// or whose probe was refused or has not answered yet.
    #[test]
    fn your_other_machine_is_listed_whatever_it_shares_or_runs() {
        let other = format!("relay:{OTHER_MAC}");
        let this = format!("relay:{THIS_MAC}");
        let probes: [(&[&str], bool, Option<&str>); 4] = [
            // Current cua-spacesd, desktop not shared: desktop features
            // unsupported, host_spaces on.
            (&["pty", "files", "host_spaces"], true, None),
            // 0.2.2 reports the desktop's features to anyone.
            (
                &["pty", "desktop_stream", "window_stream", "host_spaces"],
                true,
                None,
            ),
            // Refused or unanswered: no features at all.
            (&[], false, Some("permission denied: desktop not shared")),
            (&[], false, None),
        ];
        for (features, reachable, error) in probes {
            let spaces = vec![
                roster_row(
                    "local:space-753034eaaf",
                    "Apple-Virtual-Machine-1.local",
                    "0.1.1",
                    &["pty"],
                    true,
                    None,
                ),
                roster_row(
                    &other,
                    "Mac.localdomain",
                    "0.2.2",
                    features,
                    reachable,
                    error,
                ),
                roster_row(
                    &this,
                    "Dillons-MacBook-Pro-2.local",
                    "0.2.0",
                    &[],
                    false,
                    None,
                ),
            ];
            let listed = without_this_relay_machine(&spaces, Some(THIS_MAC));
            let view = sidebar(&listed, "", "");
            let mine = view
                .sections
                .iter()
                .find(|s| s.title == "My machines")
                .unwrap_or_else(|| panic!("no My machines for {features:?} {error:?}: {view:?}"));
            assert_eq!(
                mine.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
                [other.as_str()],
                "{features:?} {error:?}"
            );
            assert!(view.sections.iter().any(|s| s.title == "This Mac"
                && s.rows.iter().any(|r| r.id == "local:space-753034eaaf")));
        }
        // Desktop not shared but providing Spaces: its detail says so, with
        // Spaces still offered (not hidden as "shares nothing").
        let m = roster_row(
            &other,
            "Mac.localdomain",
            "0.2.2",
            &["pty", "host_spaces"],
            true,
            None,
        );
        assert_eq!(shares_desktop(&m), Some(false));
        assert_eq!(
            desktop_note(&m).as_deref(),
            Some(
                "Mac.localdomain isn\u{2019}t sharing its desktop. You can still create Spaces on it."
            )
        );
    }

    /// This Mac is left out by its exact relay machine id only: never by
    /// a prefix, another case, or the other Mac's id.
    #[test]
    fn this_mac_is_excluded_only_by_its_exact_id() {
        let spaces: Vec<Space> = [
            format!("relay:{OTHER_MAC}"),
            format!("relay:{THIS_MAC}"),
            format!("relay:{THIS_MAC}0"),
            "relay:b6953d14".to_string(),
            format!("relay:{}", THIS_MAC.to_uppercase()),
        ]
        .iter()
        .map(|id| roster_row(id, "Mac", "0.2.2", &[], true, None))
        .collect();
        let ids = |v: Vec<Space>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
        let all = ids(spaces.clone());
        let without = |own: &str| ids(without_this_relay_machine(&spaces, Some(own)));
        let mut want = all.clone();
        want.retain(|id| *id != format!("relay:{THIS_MAC}"));
        assert_eq!(without(THIS_MAC), want);
        // A prefix or a near miss of this Mac's id drops nothing else.
        assert_eq!(without("b6953d14ff94"), all);
        assert_eq!(without(&format!("relay:{THIS_MAC}")), all);
        assert_eq!(without(&format!(" {THIS_MAC}")), all);
        // Not set up as a host: nothing is dropped.
        assert_eq!(ids(without_this_relay_machine(&spaces, None)), all);
    }

    /// This machine's own relay entry is dropped by id; another machine
    /// of the same name and the Spaces this machine provides stay.
    #[test]
    fn this_machine_is_not_one_of_your_machines() {
        let mut own = machine(SHARED, true);
        own.id = "relay:aaaa01".into();
        own.name = "Dana's MacBook Pro".into();
        let mut twin = own.clone();
        twin.id = "relay:bbbb02".into();
        let mut provided = machine(SHARED, true);
        provided.id = "relay:s1".into();
        provided.host = Some("aaaa01".into());
        let spaces = vec![own, twin, provided];
        let ids = |v: Vec<Space>| v.into_iter().map(|s| s.id).collect::<Vec<_>>();
        assert_eq!(
            ids(without_this_relay_machine(&spaces, Some("aaaa01"))),
            ["relay:bbbb02", "relay:s1"]
        );
        assert_eq!(ids(without_this_relay_machine(&spaces, None)).len(), 3);
        assert_eq!(ids(without_this_relay_machine(&spaces, Some(""))).len(), 3);
    }

    /// The Spaces a machine provides, for its detail.
    #[test]
    fn hosted_rows_are_the_spaces_a_machine_provides() {
        let mut child = machine(SHARED, true);
        child.id = "relay:s1".into();
        child.name = "Linux".into();
        child.host = Some("m1".into());
        let mut other = child.clone();
        other.id = "relay:s2".into();
        other.host = Some("m9".into());
        let spaces = vec![machine(SHARED, true), child, other];
        let rows = hosted_rows(&spaces, "relay:m1", "");
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["relay:s1"]
        );
        assert!(hosted_rows(&spaces, "local:a", "").is_empty());
    }

    #[test]
    fn kind_and_architecture_facts_warn_about_emulation_on_this_mac_only() {
        let omarchy = super::super::row_to_space(
            &local_row("local:o", Some("ghcr.io/trycua/omarchy:edge"), None, None),
            0,
        );
        // Before the Space reports anything: the catalog's distribution,
        // kind and (single) platform.
        assert_eq!(system_text(&omarchy), "Omarchy");
        let d = detail_live(&omarchy, None, Some("aarch64"));
        assert_eq!(fact_of(&d, "Kind").unwrap().value, "Virtual machine");
        let arch = fact_of(&d, "Architecture").unwrap();
        assert_eq!(arch.value, "x64");
        let w = arch.warning.as_ref().expect("emulated on this Mac");
        assert_eq!(w.symbol, "exclamationmark.triangle");
        assert_eq!(
            w.help,
            "Emulated on this Mac\u{2019}s ARM processor. Performance may be degraded."
        );
        let labels: Vec<&str> = d.facts.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Status",
                "Image",
                "Identifier",
                "System",
                "Kind",
                "Architecture"
            ]
        );
        // The same platform as this Mac: no warning.
        let native = super::super::row_to_space(
            &local_row("local:u", None, Some("container"), Some("aarch64")),
            0,
        );
        let d = detail_live(&native, None, Some("arm64"));
        assert_eq!(fact_of(&d, "Kind").unwrap().value, "Container");
        let arch = fact_of(&d, "Architecture").unwrap();
        assert_eq!((arch.value.as_str(), arch.warning.as_ref()), ("ARM", None));
        // A cloud Space never warns; without the host's arch nothing does.
        let cloud =
            super::super::row_to_space(&local_row("cloud:c", None, Some("vm"), Some("amd64")), 0);
        assert!(
            fact_of(&detail_live(&cloud, None, Some("arm64")), "Architecture")
                .unwrap()
                .warning
                .is_none()
        );
        assert!(
            fact_of(&detail_live(&omarchy, None, None), "Architecture")
                .unwrap()
                .warning
                .is_none()
        );
        // Unknown kind and arch (a Space added by address): no rows.
        let direct =
            super::super::row_to_space(&local_row("direct:10.0.0.5:3211", None, None, None), 0);
        let d = detail_live(&direct, None, Some("arm64"));
        assert!(fact_of(&d, "Kind").is_none() && fact_of(&d, "Architecture").is_none());
        // What the Space reports wins over the catalog.
        let mut reported = local_row("local:o", Some("ghcr.io/trycua/omarchy:edge"), None, None);
        reported.os_name = Some("Arch Linux".into());
        let s = super::super::row_to_space(&reported, 0);
        assert_eq!(system_text(&s), "Arch Linux");
    }

    #[test]
    fn sizes_read_like_macos() {
        assert_eq!(format_size(512 * 1024 * 1024), "512 MB");
        assert_eq!(format_size(4 << 30), "4 GB");
        assert_eq!(format_size(2_254_857_830), "2.1 GB");
        assert_eq!(format_size(500 << 30), "500 GB");
        assert_eq!(format_size(2 << 40), "2 TB");
        assert_eq!(format_size(1000), "1 KB");
        assert_eq!(usage_text(5 << 30, 4 << 30), "4 GB / 4 GB");
    }

    #[test]
    fn only_the_identifier_has_a_copy_button() {
        let row = SpaceRow {
            id: "direct:127.0.0.1:34752".into(),
            name: "studio".into(),
            provider: "direct".into(),
            spacesd_version: "0.4.0".into(),
            features: vec![],
            added_at: None,
            os: None,
            os_name: None,
            reachable: true,
            error: None,
            os_pretty_name: None,
            image: None,
            image_digest: None,
            kind: None,
            arch: None,
            host: None,
            host_name: None,
            power: None,
            power_state: None,
            cloud: None,
            cloud_place: None,
            cloud_delete: None,
        };
        let d = detail(&super::super::row_to_space(&row, 0));
        // Added by address: no image row, so only the identifier copies.
        let copies: Vec<(&str, &FactCopy)> = d
            .facts
            .iter()
            .filter_map(|f| f.copy.as_ref().map(|c| (f.label.as_str(), c)))
            .collect();
        assert_eq!(copies.len(), 1);
        let (label, copy) = copies[0];
        assert_eq!(label, "Identifier");
        assert_eq!(copy.text, "direct:127.0.0.1:34752");
        assert_eq!(
            (copy.symbol.as_str(), copy.help.as_str()),
            ("doc.on.doc", "Copy")
        );
        assert_eq!(
            (copy.done_symbol.as_str(), copy.done_help.as_str()),
            ("checkmark", "Copied")
        );
        assert!(copy.confirm_ms > 0);
    }

    #[test]
    fn a_space_being_deleted_neither_streams_nor_deletes_again() {
        let row = SpaceRow {
            id: "local:demo".into(),
            name: "demo".into(),
            provider: "local".into(),
            spacesd_version: "0.4.0".into(),
            features: vec!["desktop_stream".into()],
            added_at: None,
            os: None,
            os_name: None,
            reachable: true,
            error: None,
            os_pretty_name: None,
            image: None,
            image_digest: None,
            kind: None,
            arch: None,
            host: None,
            host_name: None,
            power: None,
            power_state: None,
            cloud: None,
            cloud_place: None,
            cloud_delete: None,
        };
        let space = super::super::creating::deleting_space(&super::super::row_to_space(&row, 0));
        let d = detail(&space);
        assert_eq!(d.facts[0].value, "Deleting\u{2026}");
        assert_eq!(d.preview_text, "Deleting\u{2026}");
        assert!(!d.can_stream && !d.show_sections && d.sections.is_empty());
        assert!(d.actions.iter().all(|a| !a.enabled));
        let sb = sidebar(std::slice::from_ref(&space), "", "");
        let r = &sb.sections[0].rows[0];
        assert_eq!((r.status_text.as_str(), r.dim), ("Deleting\u{2026}", true));
        assert_eq!((r.progress, r.trailing.as_deref()), (None, None));
    }

    #[test]
    fn spaces_a_machine_provides_nest_under_it_and_delete_there() {
        let relay = |id: &str, name: &str, host: Option<&str>| {
            crate::spaces::row_to_space(
                &serde_json::from_value(serde_json::json!({
                    "id": id,
                    "name": name,
                    "provider": "relay",
                    "reachable": true,
                    "host": host,
                    "hostName": host.map(|_| "Mac mini (spare)"),
                }))
                .unwrap(),
                0,
            )
        };
        let spaces = vec![
            relay("relay:space-1", "mac-1", Some("hostmachine1")),
            relay("relay:studio0001", "Studio", None),
            relay("relay:hostmachine1", "Mac mini (spare)", None),
            relay("relay:space-2", "linux-1", Some("hostmachine1")),
            relay("relay:space-9", "orphan", Some("gone00001")),
        ];
        let v = sidebar(&spaces, "", "");
        let mine = v
            .sections
            .iter()
            .find(|s| s.title == "My machines")
            .unwrap();
        let rows: Vec<(&str, bool)> = mine
            .rows
            .iter()
            .map(|r| (r.id.as_str(), r.nested))
            .collect();
        assert_eq!(
            rows,
            [
                ("relay:studio0001", false),
                ("relay:hostmachine1", false),
                ("relay:space-1", true),
                ("relay:space-2", true),
                ("relay:space-9", false),
            ]
        );
        assert_eq!(spaces[0].detail, "On Mac mini (spare)");
        // Created by cua on that machine: Delete deletes it there.
        assert!(!added_by_address(&spaces[0]));
        assert!(added_by_address(&spaces[1]));
    }

    fn powered(id: &str, power: &str, state: Option<&str>, reachable: bool) -> Space {
        let row: SpaceRow = serde_json::from_value(serde_json::json!({
            "id": id, "name": "box", "provider": id.split(':').next().unwrap(),
            "os": "linux", "reachable": reachable, "power": power, "powerState": state,
        }))
        .unwrap();
        super::super::row_to_space(&row, 0)
    }

    fn help(s: &Space) -> Option<(String, bool, bool)> {
        power_button(s).map(|b| (b.help, b.turn_on, b.enabled))
    }

    #[test]
    fn the_power_button_says_what_a_press_does() {
        let on = |h: &str, turn_on| Some((h.to_string(), turn_on, true));
        assert_eq!(
            help(&powered("local:a", "suspend", Some("running"), true)),
            on("Suspend", false)
        );
        assert_eq!(
            help(&powered("local:a", "suspend", Some("suspended"), false)),
            on("Resume", true)
        );
        assert_eq!(
            help(&powered("local:a", "stop", Some("running"), true)),
            on("Turn off", false)
        );
        assert_eq!(
            help(&powered("local:a", "stop", Some("stopped"), false)),
            on("Turn on", true)
        );
        // A Space one of your machines provides has no recorded state: off
        // while it does not answer.
        assert_eq!(
            help(&powered("relay:space-1", "stop", None, false)),
            on("Turn on", true)
        );
        // Recorded running but not answering (booting, a broken driver):
        // it is on, so the button turns it off.
        let s = powered("local:a", "suspend", Some("running"), false);
        assert_eq!(help(&s), on("Suspend", false));
        assert_eq!(status_text(&s), "Suspended");
        // Off: the row says how, and the detail does not blame a probe.
        let off = powered("local:a", "stop", Some("stopped"), true);
        assert_eq!(
            (off.status, status_text(&off).as_str()),
            (SpaceStatus::Suspended, "Off")
        );
        assert_eq!(off.detail, "Off");
        let d = detail(&off);
        assert_eq!(
            (d.facts[0].value.as_str(), d.preview_text.as_str()),
            ("Off", "Off")
        );
        let ids: Vec<DetailActionId> = d.actions.iter().map(|a| a.id).collect();
        assert_eq!(
            ids,
            [
                DetailActionId::Teleport,
                DetailActionId::Pip,
                DetailActionId::Share,
                DetailActionId::Power,
                DetailActionId::Delete,
                DetailActionId::Open
            ]
        );
        assert_eq!(d.actions[3].symbol.as_deref(), Some(POWER_SYMBOL));
    }

    #[test]
    fn no_power_button_where_nothing_can_be_turned_off() {
        // No control (a cloud Space, one added by address).
        let mut s = powered("cloud:a", "", None, true);
        assert!(s.power.is_none() && power_button(&s).is_none());
        assert!(
            !detail(&s)
                .actions
                .iter()
                .any(|a| a.id == DetailActionId::Power)
        );
        // Being deleted, or being created.
        s = powered("local:a", "suspend", Some("running"), true);
        s.status = SpaceStatus::Deleting;
        assert!(power_button(&s).is_none());
        s = powered("local:a", "suspend", Some("running"), true);
        s.progress = Some(crate::model::SpaceProgress {
            phase: "booting".into(),
            permille: 500,
            label: "Starting".into(),
            error: None,
            credit_url: None,
            transfer: None,
            cancellable: true,
            cancelling: false,
        });
        assert!(power_button(&s).is_none());
    }

    #[test]
    fn a_power_action_waits_shows_progress_and_fails_inline() {
        use super::super::creating::{CreateAction, CreatesState, compose, reduce, settle};
        let registry = vec![powered("local:a", "suspend", Some("running"), true)];
        let start = |on| CreateAction::PowerStart {
            id: "local:a".into(),
            on,
            now: 1,
        };
        let st = reduce(&CreatesState::default(), &start(false));
        let row = |st: &CreatesState| {
            sidebar(&compose(&registry, st), "", "").sections[0].rows[0].clone()
        };
        let b = row(&st).power.unwrap();
        assert_eq!(
            (b.help.as_str(), b.busy, b.enabled),
            ("Suspending\u{2026}", true, false)
        );
        assert_eq!(row(&st).status_text, "Suspending\u{2026}");
        // A second press, or one on a Space being deleted, does nothing.
        assert_eq!(reduce(&st, &start(true)), st);
        let deleting = reduce(
            &CreatesState::default(),
            &CreateAction::DeleteStart {
                id: "local:a".into(),
                now: 1,
            },
        );
        assert_eq!(reduce(&deleting, &start(false)), deleting);
        // It fails: why, inline, and the button works again.
        let failed = reduce(
            &st,
            &CreateAction::PowerFail {
                id: "local:a".into(),
                error: "docker: not running".into(),
            },
        );
        let r = row(&failed);
        assert_eq!(
            r.power.as_ref().map(|b| (b.enabled, b.busy)),
            Some((true, false))
        );
        assert_eq!(
            r.trailing.as_deref(),
            Some("Could not turn it off: docker: not running")
        );
        assert_eq!(
            detail(&compose(&registry, &failed)[0])
                .power_error
                .as_deref(),
            Some("Could not turn it off: docker: not running")
        );
        // Done: still Suspending until the registry shows it off.
        let done = reduce(
            &st,
            &CreateAction::PowerDone {
                id: "local:a".into(),
            },
        );
        assert_eq!(settle(&done, &registry), done);
        let off = vec![powered("local:a", "suspend", Some("suspended"), false)];
        assert!(settle(&done, &off).powering.is_empty());
        // Gone from the registry: forgotten.
        assert!(settle(&done, &[]).powering.is_empty());
    }
}
