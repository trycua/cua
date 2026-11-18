// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The phone's two lists, Spaces and Computers, from the account's relay
//! directory: the pool every app of the account reads (the desktop apps list
//! the same rows through `cua-spaces`).
//!
//! The rule is the desktop's, not a second one. A relay row becomes the same
//! [`SpaceRow`] the desktop builds from it (`cua_spaces::relay_info`), becomes
//! a [`Space`] through the same [`rows_to_spaces`], and
//! [`is_machine_desktop`] decides which side it is on:
//!
//! - **A Space** is a Space a computer provides (a row with a host above it),
//!   a Space that registered itself (`space-…`), or a Space in your cloud.
//! - **A computer** is anything else a person signed in on, or set up to run
//!   Spaces: a Mac, a PC, a Linux box, a cloud host. It is never a Space. Its
//!   state is a computer's (Online, Offline, Not sharing), its line says how
//!   many Spaces it runs or that it is ready for them, and it can say a
//!   Space cannot be listed from it until it is updated.
//!
//! A Space added by hand (an address, this computer's own Spaces) is not in
//! the pool and is never here: the phone reads the directory and nothing
//! else. Phones appear in the pool as phones (a glyph by model, never "Ready
//! for Spaces"), and what a pool member can host is a capability it states
//! ([`Capabilities`]), so placement ([`offered`]) follows what each member
//! says, not what kind of device it is.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::machines;
use crate::model::{Space, SpaceOs, SpaceRow, SpaceStatus};
use crate::spaces;
use crate::util::collate;

/// The relay row keys the directory carries in `meta` (kept equal to
/// `cua-host` and `cua-sandbox-core` by `tests/phone_directory_drift.rs`).
pub mod meta {
    pub const PROVIDES_SPACES: &str = "cua.host.spaces";
    pub const OS: &str = "cua.host.os";
    pub const ARCH: &str = "cua.host.arch";
    pub const CLOUD_HOST: &str = "cua.host.cloud";
    pub const CLOUD_PROVIDER: &str = "cua.cloud.provider";
    pub const CLOUD_PLACE: &str = "cua.cloud.place";
    /// A computer's daemon states it can answer a phone's requests (it
    /// serves the phone's calls when the person turns the phone app on).
    pub const PHONE_FEED: &str = "cua.host.phone";
}

/// What a pool member can host, as it states it (additive and versioned: a
/// version this core does not know hosts nothing). A phone states none today;
/// a phone that one day runs terminal-only agent Spaces would state
/// `images: ["agent-home"]` and need no new protocol.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Capabilities {
    /// Capability format version (1).
    pub v: u32,
    /// It can run Spaces for other members.
    pub can_host_spaces: bool,
    /// The images it will run: names it knows (`agent-home`), or `*` for
    /// any. Empty with `can_host_spaces` runs none.
    pub images: Vec<String>,
    /// The kinds of Space it runs (`container`, `vm`); empty is any of the
    /// images it lists.
    pub kinds: Vec<String>,
    /// It has a screen to run desktop Spaces on.
    pub desktop: bool,
}

/// The latest capability format this core reads.
pub const CAPABILITIES_VERSION: u32 = 1;

/// A relay machine row, as `GET /v1/machines` answers it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RelayRow {
    pub id: String,
    pub name: String,
    pub online: bool,
    pub sharing: bool,
    pub version: String,
    pub host: Option<String>,
    pub role: String,
    pub meta: BTreeMap<String, String>,
    /// `false` for a computer a stolen token could have planted: listed as
    /// "new" until an enrolled device confirms it, and never used meanwhile.
    pub confirmed: bool,
}

/// What [`view`] reads.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DirectoryInput {
    /// The account's relay rows; `None` until the first read answered.
    pub machines: Option<Vec<RelayRow>>,
    /// The account's enrolled devices (phones are listed from these).
    pub devices: Vec<DeviceRow>,
    /// The read failed: the relay's words, or empty.
    pub error: String,
    /// The read is still running (some rows may not be in yet).
    pub reading: bool,
    /// Unix milliseconds.
    pub now: i64,
}

/// One enrolled device of the account.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub phone: bool,
    pub state: String,
    pub current: bool,
    pub last_seen: Option<u64>,
    pub capabilities: Option<Capabilities>,
}

/// One computer (or phone) of the pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComputerRow {
    /// The relay machine id, or the device id of a phone.
    pub id: String,
    pub name: String,
    /// `mac`, `windows`, `linux`, `cloud`, `iphone`, `ipad` or `phone`: the
    /// glyph.
    pub kind: String,
    /// `Online`, `Offline` or `Not sharing`.
    pub state: String,
    pub online: bool,
    /// The row's one muted line: "2 Spaces", "Ready for Spaces", "No Spaces".
    pub line: String,
    /// A plain sentence when this computer's Spaces cannot be listed from
    /// here, or empty.
    pub note: String,
    /// It runs Spaces for the pool (the "Let other computers create Spaces
    /// here" switch), as its relay row says.
    pub hosts_spaces: bool,
    pub spaces: u32,
    pub is_phone: bool,
    /// This phone itself (the one reading).
    pub current: bool,
    /// New and unconfirmed: shown, never used.
    pub unconfirmed: bool,
    /// What it states it can host.
    pub capabilities: Capabilities,
}

/// One Space of the pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceOut {
    /// `relay:<machine id>`.
    pub id: String,
    /// The relay machine id.
    pub machine: String,
    pub name: String,
    /// The computer it runs on ("" when the directory does not list it).
    pub host: String,
    pub host_name: String,
    /// `Running`, `Stopped`, `Suspended`, `Starting`, `Offline` or
    /// `Not sharing`: the desktop's words for the same Space.
    pub state: String,
    pub online: bool,
    /// `macos`, `windows`, `linux` or empty.
    pub os: String,
    /// "macOS VM", "Linux", "Space in AWS": what it is.
    pub kind_label: String,
    /// `owner`, `editor` or `viewer`.
    pub role: String,
    /// Where it runs, in words ("On Studio Mac", "AWS us-west-2").
    pub detail: String,
}

/// The two lists and their words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryView {
    pub computers: Vec<ComputerRow>,
    pub spaces: Vec<SpaceOut>,
    /// "Reaching 3 computers…", "Loading your Spaces…", or none.
    pub reaching_line: Option<String>,
    /// What the Spaces list says when it is empty, or none when it is not.
    pub empty: Option<EmptyView>,
    /// The relay's failure in plain words, or none.
    pub error_line: Option<String>,
}

/// An empty list's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyView {
    pub title: String,
    pub body: String,
}

fn os_of(row: &RelayRow) -> SpaceOs {
    match row
        .meta
        .get(meta::OS)
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some(o) if o.starts_with("mac") => SpaceOs::Macos,
        Some(o) if o.starts_with("win") => SpaceOs::Windows,
        Some(o) if o.starts_with("lin") => SpaceOs::Linux,
        _ => SpaceOs::Unknown,
    }
}

fn cloud_of(row: &RelayRow) -> Option<&str> {
    row.meta
        .get(meta::CLOUD_PROVIDER)
        .map(String::as_str)
        .filter(|p| !p.is_empty())
}

fn provided(row: &RelayRow) -> bool {
    row.host.as_deref().is_some_and(|h| !h.is_empty())
}

/// The row the desktop's SDK builds from a relay row (`relay_info`, the
/// parts that decide a list), so the same rules read it.
pub fn space_row(row: &RelayRow, host_name: Option<&str>) -> SpaceRow {
    let host = row.host.clone().filter(|h| !h.is_empty());
    let os = os_of(row);
    SpaceRow {
        hidden: false,
        parent: None,
        id: format!("relay:{}", row.id),
        name: if row.name.is_empty() {
            row.id.clone()
        } else {
            row.name.clone()
        },
        display_name: None,
        provider: "relay".into(),
        spacesd_version: row.version.clone(),
        features: vec![],
        added_at: None,
        os: (os != SpaceOs::Unknown).then_some(os),
        os_name: None,
        os_pretty_name: None,
        image: None,
        image_digest: None,
        kind: None,
        arch: row.meta.get(meta::ARCH).cloned(),
        reachable: row.online,
        // A computer whose owner stopped sharing it answers, and says so.
        error: (row.online && !row.sharing).then(|| "stopped sharing".to_string()),
        // A Space a computer provides is turned off and on by that computer.
        power: host.is_some().then(|| "stop".to_string()),
        host_name: host_name.map(str::to_owned),
        host,
        power_state: None,
        cloud: cloud_of(row).map(str::to_owned),
        cloud_place: row.meta.get(meta::CLOUD_PLACE).cloned(),
        cloud_delete: None,
        stop_reason: None,
        sharing: None,
    }
}

/// Whether `row` is a Space (not a computer): the desktop's rule. A Space has
/// a host above it, registered itself (`space-…`), or lives in a cloud.
pub fn is_space(row: &RelayRow) -> bool {
    provided(row) || cloud_of(row).is_some() || row.id.starts_with("space-")
}

fn space_state(s: &Space, row: &RelayRow) -> &'static str {
    if !row.sharing && row.online {
        return "Not sharing";
    }
    match s.status {
        SpaceStatus::Running | SpaceStatus::Local | SpaceStatus::Approval => "Running",
        SpaceStatus::Provisioning => "Starting",
        SpaceStatus::Deleting => "Deleting",
        SpaceStatus::Suspended => match &s.power {
            // Turned off on purpose, and the desktop says "Stopped".
            Some(p) if p.off && provided(row) => "Stopped",
            _ => "Offline",
        },
    }
}

fn kind_label(row: &RelayRow, os: SpaceOs) -> String {
    if let Some(c) = cloud_of(row) {
        return format!("Space in {}", cloud_word(c));
    }
    let word = match os {
        SpaceOs::Macos => "macOS",
        SpaceOs::Windows => "Windows",
        SpaceOs::Linux => "Linux",
        SpaceOs::Unknown => "",
    };
    match (word.is_empty(), provided(row)) {
        (true, _) => "Space".into(),
        (false, true) => format!("{word} VM"),
        (false, false) => word.into(),
    }
}

fn cloud_word(c: &str) -> String {
    match c {
        "aws" => "AWS".into(),
        "gcp" => "Google Cloud".into(),
        "azure" => "Azure".into(),
        "hetzner" => "Hetzner".into(),
        "modal" => "Modal".into(),
        other => other.to_string(),
    }
}

fn role_of(row: &RelayRow) -> &'static str {
    if row.role == "shared" {
        "editor"
    } else if row.role == "viewer" {
        "viewer"
    } else {
        "owner"
    }
}

fn computer_kind(row: &RelayRow) -> &'static str {
    if cloud_of(row).is_some() || row.meta.contains_key(meta::CLOUD_HOST) {
        return "cloud";
    }
    match os_of(row) {
        SpaceOs::Windows => "windows",
        SpaceOs::Linux => "linux",
        _ => "mac",
    }
}

/// `iphone`, `ipad` or `phone` for a phone, from the name it reports (the
/// platform alone says iOS or Android, not which).
pub fn phone_kind(name: &str, platform: &str) -> &'static str {
    let n = name.to_ascii_lowercase();
    if n.contains("ipad") {
        "ipad"
    } else if n.contains("iphone") || platform.eq_ignore_ascii_case("ios") {
        "iphone"
    } else {
        "phone"
    }
}

/// Builds both lists.
pub fn view(input: &DirectoryInput) -> DirectoryView {
    let rows: &[RelayRow] = input.machines.as_deref().unwrap_or(&[]);
    let names: BTreeMap<&str, &str> = rows
        .iter()
        .map(|r| {
            (
                r.id.as_str(),
                if r.name.is_empty() {
                    r.id.as_str()
                } else {
                    r.name.as_str()
                },
            )
        })
        .collect();
    // The Spaces: every row the rule says is one, as the desktop lists it.
    let mut spaces_out: Vec<SpaceOut> = Vec::new();
    let mut by_host: BTreeMap<String, u32> = BTreeMap::new();
    for row in rows {
        let host = row.host.clone().filter(|h| !h.is_empty());
        let sr = space_row(row, host.as_deref().and_then(|h| names.get(h).copied()));
        let space = spaces::row_to_space(&sr, input.now);
        if !is_space(row) {
            continue;
        }
        if let Some(h) = &host {
            *by_host.entry(h.clone()).or_default() += 1;
        }
        let os = space.os;
        spaces_out.push(SpaceOut {
            id: space.id.clone(),
            machine: row.id.clone(),
            name: space.name.clone(),
            host: host.clone().unwrap_or_default(),
            host_name: host
                .as_deref()
                .and_then(|h| names.get(h))
                .map(|n| n.to_string())
                .unwrap_or_default(),
            state: space_state(&space, row).into(),
            online: row.online,
            os: match os {
                SpaceOs::Unknown => String::new(),
                o => o.as_str().into(),
            },
            kind_label: kind_label(row, os),
            role: role_of(row).into(),
            detail: space.detail.clone(),
        });
    }
    // Running first, then by name: how the desktop orders what is on.
    spaces_out.sort_by(|a, b| {
        let live = |s: &SpaceOut| u8::from(s.state != "Running");
        live(a)
            .cmp(&live(b))
            .then_with(|| collate(&a.name, &b.name))
    });
    // The computers: the rows that are not Spaces, then the account's phones.
    let mut computers: Vec<ComputerRow> = Vec::new();
    for row in rows {
        if is_space(row) {
            continue;
        }
        let stopped = row.online && !row.sharing;
        let hosts = row.meta.get(meta::PROVIDES_SPACES).map(String::as_str) == Some("on");
        let n = by_host.get(&row.id).copied().unwrap_or(0);
        let line = if !row.online {
            String::new()
        } else if hosts && !stopped {
            if n == 0 {
                "Ready for Spaces".into()
            } else {
                machines::spaces_label(n)
            }
        } else if stopped {
            "Signed in, not sharing".into()
        } else {
            String::new()
        };
        // A computer that runs Spaces for the pool and has not said it can
        // answer a phone: its Spaces are not listed, and the person is told.
        let note = if hosts && row.online && !stopped && !row.meta.contains_key(meta::PHONE_FEED) {
            "Update Cua Spaces on this computer to see its Spaces here.".to_string()
        } else {
            String::new()
        };
        computers.push(ComputerRow {
            id: row.id.clone(),
            name: if row.name.is_empty() {
                row.id.clone()
            } else {
                row.name.clone()
            },
            kind: computer_kind(row).into(),
            state: machines::status_word(row.online, stopped).into(),
            online: row.online,
            line,
            note,
            hosts_spaces: hosts,
            spaces: n,
            is_phone: false,
            current: false,
            unconfirmed: !row.confirmed,
            capabilities: Capabilities {
                v: CAPABILITIES_VERSION,
                can_host_spaces: hosts,
                images: if hosts { vec!["*".into()] } else { vec![] },
                kinds: vec![],
                desktop: true,
            },
        });
    }
    for d in input
        .devices
        .iter()
        .filter(|d| d.phone && d.state != "revoked")
    {
        let now_s = (input.now.max(0) / 1000) as u64;
        let online = d
            .last_seen
            .is_some_and(|t| now_s.saturating_sub(t) < machines::DEVICE_ONLINE_SECS);
        let caps = d.capabilities.clone().unwrap_or_default();
        computers.push(ComputerRow {
            id: d.id.clone(),
            name: if d.name.is_empty() {
                "Phone".into()
            } else {
                d.name.clone()
            },
            kind: phone_kind(&d.name, &d.platform).into(),
            state: machines::status_word(online || d.current, false).into(),
            online: online || d.current,
            // A phone does not run Spaces unless it says so.
            line: if usable(&caps) {
                "Ready for Spaces".into()
            } else {
                String::new()
            },
            note: String::new(),
            hosts_spaces: usable(&caps),
            spaces: 0,
            is_phone: true,
            current: d.current,
            unconfirmed: false,
            capabilities: caps,
        });
    }
    // Online first, then by name; phones last among equals.
    computers.sort_by(|a, b| {
        let off = |c: &ComputerRow| u8::from(!c.online);
        off(a)
            .cmp(&off(b))
            .then(a.is_phone.cmp(&b.is_phone))
            .then_with(|| collate(&a.name, &b.name))
    });
    let reaching_line = if input.machines.is_none() && input.error.is_empty() {
        Some("Loading your Spaces…".to_string())
    } else if input.reading {
        let n = computers.iter().filter(|c| !c.is_phone && c.online).count();
        Some(match n {
            0 => "Reaching your computers…".to_string(),
            1 => "Reaching 1 computer…".to_string(),
            n => format!("Reaching {n} computers…"),
        })
    } else {
        None
    };
    let error_line = (!input.error.is_empty()).then(|| input.error.clone());
    let complete = input.machines.is_some() && !input.reading && input.error.is_empty();
    let empty = (complete && spaces_out.is_empty()).then(|| {
        let real: Vec<&ComputerRow> = computers.iter().filter(|c| !c.is_phone).collect();
        if real.is_empty() {
            EmptyView {
                title: "No computers yet".into(),
                body: "Open Cua Spaces on a Mac or PC and sign in with this account. It shows up here.".into(),
            }
        } else if !real.iter().any(|c| c.hosts_spaces) {
            EmptyView {
                title: "No Spaces yet".into(),
                body: "None of your computers runs Spaces for your account. In Cua Spaces on one, turn on Let other computers create Spaces here.".into(),
            }
        } else {
            EmptyView {
                title: "No Spaces yet".into(),
                body: "Make a Space from Cua Spaces on a computer or start a task here.".into(),
            }
        }
    });
    DirectoryView {
        computers,
        spaces: spaces_out,
        reaching_line,
        empty,
        error_line,
    }
}

fn usable(c: &Capabilities) -> bool {
    c.v == CAPABILITIES_VERSION && c.can_host_spaces && !c.images.is_empty()
}

// ---- placement ---------------------------------------------------------

/// What a new Space needs of the computer that runs it.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Need {
    /// The image (`agent-home`, `ghcr.io/trycua/linux:24.04`); empty is any.
    pub image: String,
    /// `container` or `vm`; empty is any.
    pub kind: String,
    /// It needs a desktop (a screen to run on).
    pub desktop: bool,
}

/// One member of the pool a Space could be placed on.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Member {
    pub id: String,
    pub online: bool,
    /// The "Let other computers create Spaces here" switch.
    pub hosts_spaces: bool,
    pub capabilities: Capabilities,
}

/// The members `need` can be placed on, in `members`' order. Follows what
/// each member states (and its switch), never its kind: a computer that
/// states nothing is not offered, a phone that states `agent-home` is offered
/// for an agent-home Space and not for a desktop one.
pub fn offered(members: &[Member], need: &Need) -> Vec<String> {
    members
        .iter()
        .filter(|m| m.online && m.hosts_spaces && fits(&m.capabilities, need))
        .map(|m| m.id.clone())
        .collect()
}

fn fits(c: &Capabilities, need: &Need) -> bool {
    if c.v != CAPABILITIES_VERSION || !c.can_host_spaces {
        return false;
    }
    let image_ok = need.image.is_empty()
        || c.images
            .iter()
            .any(|i| i == "*" || i.eq_ignore_ascii_case(&need.image));
    let any_image = !c.images.is_empty();
    let kind_ok = need.kind.is_empty()
        || c.kinds.is_empty()
        || c.kinds.iter().any(|k| k.eq_ignore_ascii_case(&need.kind));
    any_image && image_ok && kind_ok && (!need.desktop || c.desktop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spaces::is_machine_desktop;
    use serde_json::json;

    fn input(machines: serde_json::Value) -> DirectoryInput {
        serde_json::from_value(json!({"machines": machines, "now": 1_793_000_000_000i64})).unwrap()
    }

    /// An account like the owner's: a Mac that signed in and does not run
    /// Spaces, a Mac that runs them (with two Spaces), a cloud host, a Space
    /// registered by itself, and a Space of a cloud.
    fn pool() -> serde_json::Value {
        json!([
            {"id": "mac-plain", "name": "Dillons-MacBook-Pro-2.local", "online": true, "sharing": false, "role": "owner", "meta": {}},
            {"id": "mac-host", "name": "Studio Mac", "online": true, "sharing": true, "role": "owner",
             "meta": {"cua.host.spaces": "on", "cua.host.os": "macos", "cua.host.phone": "1"}},
            {"id": "old-host", "name": "updmac-vm-host", "online": true, "sharing": true, "role": "owner",
             "meta": {"cua.host.spaces": "on", "cua.host.os": "macos"}},
            {"id": "cloud-host", "name": "ip-172-31-46-222", "online": false, "sharing": true, "role": "owner",
             "meta": {"cua.host.spaces": "on", "cua.host.os": "linux", "cua.host.cloud": "aws"}},
            {"id": "space-e935618745", "name": "space-e935618745", "online": true, "sharing": true, "role": "owner", "host": "mac-host", "meta": {}},
            {"id": "space-654aaacf10", "name": "builder", "online": false, "sharing": true, "role": "owner", "host": "mac-host", "meta": {}},
            {"id": "space-a7df9eb765", "name": "self-registered", "online": true, "sharing": true, "role": "owner", "meta": {}},
            {"id": "space-cc00000001", "name": "modal-box", "online": true, "sharing": true, "role": "owner",
             "meta": {"cua.cloud.provider": "modal"}},
        ])
    }

    #[test]
    fn a_computer_is_never_a_space_and_a_space_is_never_a_computer() {
        let v = view(&input(pool()));
        let spaces: Vec<&str> = v.spaces.iter().map(|s| s.machine.as_str()).collect();
        let computers: Vec<&str> = v.computers.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            spaces,
            [
                "space-cc00000001",
                "space-a7df9eb765",
                "space-e935618745",
                "space-654aaacf10"
            ],
            "running first, then by name"
        );
        assert_eq!(
            computers,
            ["mac-plain", "mac-host", "old-host", "cloud-host"],
            "online first, then by name"
        );
        for c in &computers {
            assert!(!spaces.contains(c), "{c} is on both lists");
        }
        // The plain Mac that does not share its desktop is a computer with a
        // state, not a Space called by its hostname.
        let plain = v.computers.iter().find(|c| c.id == "mac-plain").unwrap();
        assert_eq!(
            (plain.state.as_str(), plain.line.as_str()),
            ("Not sharing", "Signed in, not sharing")
        );
        assert!(!v.spaces.iter().any(|s| s.name.contains("MacBook")));
    }

    #[test]
    fn spaces_have_the_desktops_words_and_say_where_they_run() {
        let v = view(&input(pool()));
        let by = |m: &str| v.spaces.iter().find(|s| s.machine == m).unwrap();
        assert_eq!(by("space-e935618745").state, "Running");
        assert_eq!(by("space-e935618745").host_name, "Studio Mac");
        assert_eq!(by("space-e935618745").detail, "On Studio Mac");
        // A provided Space that does not answer is off, as the desktop says.
        assert_eq!(by("space-654aaacf10").state, "Stopped");
        assert_eq!(by("space-654aaacf10").kind_label, "Space");
        assert_eq!(by("space-cc00000001").kind_label, "Space in Modal");
        assert_eq!(by("space-a7df9eb765").host, "");
    }

    #[test]
    fn a_computer_says_how_many_spaces_it_runs_or_that_it_is_ready_or_what_to_update() {
        let v = view(&input(pool()));
        let c = |id: &str| v.computers.iter().find(|c| c.id == id).unwrap();
        assert_eq!(c("mac-host").line, "2 Spaces");
        assert_eq!(c("mac-host").note, "");
        assert_eq!(c("old-host").line, "Ready for Spaces");
        assert_eq!(
            c("old-host").note,
            "Update Cua Spaces on this computer to see its Spaces here.",
            "a computer whose daemon has not said it answers a phone is told, not faked"
        );
        // Offline: a state, no line, no promise.
        assert_eq!(
            (
                c("cloud-host").state.as_str(),
                c("cloud-host").line.as_str()
            ),
            ("Offline", "")
        );
        assert_eq!(c("cloud-host").kind, "cloud");
    }

    #[test]
    fn nothing_is_faked_while_loading_empty_or_failed() {
        let loading: DirectoryInput = serde_json::from_value(json!({"now": 0})).unwrap();
        let v = view(&loading);
        assert_eq!(v.reaching_line.as_deref(), Some("Loading your Spaces…"));
        assert!(v.empty.is_none() && v.spaces.is_empty() && v.computers.is_empty());
        let failed: DirectoryInput = serde_json::from_value(
            json!({"error": "The relay could not be reached. Check the connection.", "now": 0}),
        )
        .unwrap();
        let v = view(&failed);
        assert_eq!(
            v.error_line.as_deref(),
            Some("The relay could not be reached. Check the connection.")
        );
        assert!(v.empty.is_none(), "a failed read is not an empty account");
        // Empty, with no computer, and with computers that run no Spaces.
        let none = view(&input(json!([])));
        assert_eq!(none.empty.unwrap().title, "No computers yet");
        let plain = view(&input(
            json!([{"id": "m1", "name": "Studio", "online": true, "sharing": true, "meta": {}}]),
        ));
        assert!(
            plain
                .empty
                .unwrap()
                .body
                .contains("Let other computers create Spaces here")
        );
        let ready = view(&input(
            json!([{"id": "m1", "name": "Studio", "online": true, "sharing": true, "meta": {"cua.host.spaces": "on"}}]),
        ));
        assert_eq!(
            ready.empty.unwrap().body,
            "Make a Space from Cua Spaces on a computer or start a task here."
        );
        // Still reading: never empty.
        let mut reading = input(
            json!([{"id": "m1", "name": "Studio", "online": true, "sharing": true, "meta": {}}]),
        );
        reading.reading = true;
        let v = view(&reading);
        assert!(v.empty.is_none());
        assert_eq!(v.reaching_line.as_deref(), Some("Reaching 1 computer…"));
    }

    #[test]
    fn the_phone_list_is_the_desktops_list_for_the_same_account() {
        // The same rows through the desktop's own path: rows -> Spaces ->
        // "is it a machine's own desktop?", and the Machines page's rows.
        let p = pool();
        let rows: Vec<RelayRow> = serde_json::from_value(p.clone()).unwrap();
        let names: BTreeMap<&str, &str> = rows
            .iter()
            .map(|r| (r.id.as_str(), r.name.as_str()))
            .collect();
        let desktop: Vec<Space> = rows
            .iter()
            .map(|r| {
                let host = r.host.as_deref();
                spaces::row_to_space(&space_row(r, host.and_then(|h| names.get(h).copied())), 0)
            })
            .collect();
        let mut desktop_spaces: Vec<String> = desktop
            .iter()
            .filter(|s| !is_machine_desktop(s))
            .map(|s| s.id.clone())
            .collect();
        let v = view(&input(p));
        let mut phone_spaces: Vec<String> = v.spaces.iter().map(|s| s.id.clone()).collect();
        desktop_spaces.sort();
        phone_spaces.sort();
        assert_eq!(phone_spaces, desktop_spaces, "the same Spaces");
        // The computers are the Machines page's rows (without this machine).
        let page = machines::page_rows(&machines::MachinesPageInput {
            spaces: desktop,
            ..Default::default()
        });
        let mut page_ids: Vec<String> = page
            .iter()
            .filter(|r| !r.current)
            .map(|r| r.id.clone())
            .collect();
        page_ids.sort();
        let mut computer_ids: Vec<String> = v.computers.iter().map(|c| c.id.clone()).collect();
        computer_ids.sort();
        // The page lists a computer through its Space rows or its hosts: the
        // ones that provide Spaces and own desktops, which is all of them
        // here.
        assert_eq!(computer_ids, page_ids, "the same computers");
        // The hosts' Space counts agree too.
        for r in page.iter().filter(|r| !r.current) {
            let c = v.computers.iter().find(|c| c.id == r.id).unwrap();
            assert_eq!(c.spaces, r.spaces, "{}", r.id);
            assert_eq!(c.state, r.status, "{}", r.id);
        }
    }

    #[test]
    fn a_space_added_by_hand_is_not_in_the_pool_and_so_not_on_the_phone() {
        // The desktop's own list has rows the pool does not: this computer's
        // Spaces and an address typed in. The phone reads relay rows only, so
        // such a row cannot reach its list; and a relay row for a `direct:`
        // style id cannot pass for a Space.
        let v = view(&input(json!([
            {"id": "direct:10.0.0.5:3211", "name": "typed in", "online": true, "sharing": true, "meta": {}}
        ])));
        assert!(v.spaces.is_empty(), "{:?}", v.spaces);
    }

    #[test]
    fn phones_are_in_the_pool_as_phones() {
        let mut inp = input(pool());
        inp.devices = serde_json::from_value(json!([
            {"id": "dev_aaaa", "name": "Dillon's iPhone", "platform": "ios", "phone": true, "state": "enrolled", "current": true},
            {"id": "dev_bbbb", "name": "Kitchen iPad", "platform": "ios", "phone": true, "state": "enrolled", "lastSeen": 1_792_999_900u64},
            {"id": "dev_cccc", "name": "Lost phone", "platform": "ios", "phone": true, "state": "revoked"},
            {"id": "dev_dddd", "name": "laptop", "platform": "macos", "phone": false, "state": "enrolled"},
        ]))
        .unwrap();
        let v = view(&inp);
        let phones: Vec<&ComputerRow> = v.computers.iter().filter(|c| c.is_phone).collect();
        assert_eq!(
            phones.len(),
            2,
            "a revoked phone and a laptop are not phones in the list"
        );
        let me = phones.iter().find(|p| p.current).unwrap();
        assert_eq!(
            (me.kind.as_str(), me.name.as_str()),
            ("iphone", "Dillon's iPhone")
        );
        assert!(
            me.line.is_empty(),
            "a phone is never Ready for Spaces unless it states it"
        );
        assert_eq!(
            phones.iter().find(|p| p.id == "dev_bbbb").unwrap().kind,
            "ipad"
        );
        assert!(!v.spaces.iter().any(|s| s.machine.starts_with("dev_")));
        assert_ne!(me.name, "This phone");
    }

    fn member(id: &str, hosts: bool, caps: Capabilities) -> Member {
        Member {
            id: id.into(),
            online: true,
            hosts_spaces: hosts,
            capabilities: caps,
        }
    }

    #[test]
    fn placement_follows_what_a_member_states() {
        let mac = Capabilities {
            v: 1,
            can_host_spaces: true,
            images: vec!["*".into()],
            kinds: vec![],
            desktop: true,
        };
        let phone_none = Capabilities {
            v: 1,
            ..Default::default()
        };
        let phone_terminal = Capabilities {
            v: 1,
            can_host_spaces: true,
            images: vec!["agent-home".into()],
            kinds: vec!["container".into()],
            desktop: false,
        };
        let members = vec![
            member("mac", true, mac),
            member("phone-none", true, phone_none.clone()),
            member("phone-terminal", true, phone_terminal.clone()),
        ];
        let desktop = Need {
            image: "ghcr.io/trycua/macos:15".into(),
            kind: "vm".into(),
            desktop: true,
        };
        let home = Need {
            image: "agent-home".into(),
            kind: "container".into(),
            desktop: false,
        };
        // A phone that states nothing is offered nothing; one that states
        // agent-home is offered for an agent-home Space and not a desktop one.
        assert_eq!(offered(&members, &desktop), ["mac"]);
        assert_eq!(offered(&members, &home), ["mac", "phone-terminal"]);
        assert_eq!(
            offered(&members, &Need::default()),
            ["mac", "phone-terminal"]
        );
        // The switch and being online still decide: "Let other computers
        // create Spaces here" off, or offline, is not offered whatever it can.
        let mut off = member("phone-terminal", false, phone_terminal.clone());
        assert!(offered(&[off.clone()], &home).is_empty());
        off.hosts_spaces = true;
        off.online = false;
        assert!(offered(&[off], &home).is_empty());
        // A format this core does not know hosts nothing (fail closed).
        let future = Capabilities {
            v: 2,
            can_host_spaces: true,
            images: vec!["*".into()],
            kinds: vec![],
            desktop: true,
        };
        assert!(offered(&[member("x", true, future)], &Need::default()).is_empty());
        // A fixture member with can_host_spaces=true and no images runs nothing.
        let empty = Capabilities {
            v: 1,
            can_host_spaces: true,
            ..Default::default()
        };
        assert!(offered(&[member("y", true, empty)], &Need::default()).is_empty());
    }

    #[test]
    fn the_core_answers_through_the_json_entry_point() {
        let v = crate::dispatch::call_value(
            "directory.view",
            json!({"input": {"machines": pool(), "now": 1}}),
        )
        .unwrap();
        assert_eq!(v["spaces"].as_array().unwrap().len(), 4);
        assert_eq!(v["computers"][0]["id"], "mac-plain");
        let o = crate::dispatch::call_value(
            "placement.offered",
            json!({"members": [{"id": "m", "online": true, "hostsSpaces": true,
                                "capabilities": {"v": 1, "canHostSpaces": true, "images": ["*"], "desktop": true}}],
                   "need": {"desktop": true}}),
        )
        .unwrap();
        assert_eq!(o, json!(["m"]));
    }
}
