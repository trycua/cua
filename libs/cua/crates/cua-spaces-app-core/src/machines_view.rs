// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Machines page as every UI draws it: the list in groups ("Your
//! machines", a collapsed "Offline for 30+ days", "Team machines"), one
//! muted line per row, and each machine's page (its name and rename, the
//! Provide Spaces and Share this desktop switches, the Spaces on it as
//! links, New Space here, Details, and Remove from account).
//!
//! The page is about the hardware. Opening, starting, stopping, teleporting
//! to or dropping files on a Space belong to the Space, so a machine's
//! Space rows only lead to the Space's own page.
//!
//! [`view`] reads the rows [`crate::machines::page_rows`] lists, what the
//! relay says of each machine, This machine's host state and a few facts
//! the shell knows about the computer it runs on. The SwiftUI app, the web
//! UI in the Mac app and in Electron all draw this one view.

use serde::{Deserialize, Serialize};

use crate::host::{HostActionId, HostConfirm, HostState};
use crate::machines::{self, MachinePageRow, MachinesPageInput};
use crate::model::{Space, SpaceOs, SpaceProvider, SpaceStatus};

/// A machine offline for this long goes under "Offline for 30+ days".
pub const OLD_AFTER_SECS: u64 = 30 * 24 * 60 * 60;

/// What the relay says of one of the account's machines (the SDK's
/// `RelayMachine`, the fields the page reads).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewRelay {
    /// Machine id.
    pub id: String,
    /// `owner`, or `shared` for a machine someone shares with you.
    #[serde(default)]
    pub role: String,
    /// The team you reach it through, for a team machine.
    #[serde(default)]
    pub team: Option<String>,
    /// Its owner's email, when the relay says.
    #[serde(default)]
    pub owner_email: Option<String>,
    /// It creates Spaces for your devices (`None`: it did not say).
    #[serde(default)]
    pub provides_spaces: Option<bool>,
}

/// What the shell knows about the computer it runs on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewAbout {
    /// Its name ("Dillon's MacBook Pro").
    #[serde(default)]
    pub computer_name: String,
    /// The app's version ("0.9.1").
    #[serde(default)]
    pub app_version: String,
    /// Its system and version ("macOS 26.5.2").
    #[serde(default)]
    pub os_version: String,
}

/// What [`view`] reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewInput {
    /// What the rows are listed from ([`machines::page_rows`]).
    #[serde(default)]
    pub page: MachinesPageInput,
    /// The account's relay machines (none while signed out).
    #[serde(default)]
    pub relay: Vec<MachinesViewRelay>,
    /// This machine's host state (`None` while it loads).
    #[serde(default)]
    pub host: Option<HostState>,
    /// This computer.
    #[serde(default)]
    pub about: MachinesViewAbout,
    /// The rows when the shell already listed them (an older host's rows,
    /// merged by the web UI); `None`: [`machines::page_rows`] of `page`.
    #[serde(default)]
    pub rows: Option<Vec<MachinePageRow>>,
}

/// One of the two switches on a machine's page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewToggle {
    /// `spaces` or `desktop`.
    pub id: String,
    /// "Provide Spaces", "Share this desktop".
    pub label: String,
    /// One line under it.
    pub help: String,
    /// On now.
    pub on: bool,
    /// Changed here (This machine); another machine's are read-only.
    pub editable: bool,
    /// Can be flipped now.
    pub enabled: bool,
    /// What flipping it runs, in order (the host's actions).
    #[serde(default)]
    pub actions: Vec<HostActionId>,
    /// Before This machine is set up: flipping it opens the setup form
    /// with this profile (`desktop` or `spare`).
    #[serde(default)]
    pub setup: Option<String>,
}

/// A Space on a machine: a link to the Space's own page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewSpace {
    /// The Space's id (its page).
    pub id: String,
    /// Its name.
    pub name: String,
    /// Its system ("Ubuntu 24.04.5 LTS", "macOS").
    pub os: String,
    /// Its status word ("Running", "Stopped").
    pub status: String,
    /// Running (the green dot).
    pub running: bool,
}

/// New Space here: the wizard with this machine chosen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewNewSpace {
    /// "New Space here".
    pub label: String,
    /// The wizard's placement (`local`, `host:<id>`).
    pub on: String,
    /// Can be pressed now.
    pub enabled: bool,
    /// Why not, when it cannot.
    #[serde(default)]
    pub help: Option<String>,
}

/// Rename: what the new name is saved on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewRename {
    /// `host` (This machine's host setup), `machine` (another machine of
    /// yours on the relay) or `device` (one of your devices).
    pub target: String,
    /// The id it is saved under (the relay machine's or the device's).
    pub id: String,
    /// The dialog's title.
    pub title: String,
    /// Its confirming button.
    pub confirm_label: String,
}

/// Remove from account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewRemove {
    /// "Remove from account".
    pub label: String,
    /// Can be pressed now.
    pub enabled: bool,
    /// Why not, when it cannot.
    #[serde(default)]
    pub help: Option<String>,
    /// This machine: its host setup is removed (the host's `remove`).
    pub host: bool,
    /// Another machine: it leaves the relay directory (`relay:<id>`).
    #[serde(default)]
    pub machine: Option<String>,
    /// The device on it, revoked too.
    #[serde(default)]
    pub device: Option<String>,
    /// Asked first.
    pub confirm: HostConfirm,
}

/// A label and its value under Details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewFact {
    /// "OS", "App", "Connection", "Service".
    pub label: String,
    /// Its value.
    pub value: String,
}

/// One machine: its list row and its page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewMachine {
    /// The row's id ([`MachinePageRow::id`]).
    pub id: String,
    /// Its name.
    pub name: String,
    /// "This Mac" on the machine this app runs on.
    #[serde(default)]
    pub badge: Option<String>,
    /// The row's one muted line ("2 Spaces running", "Ready for Spaces",
    /// "Offline · last seen 9 days ago").
    pub line: String,
    /// Usable now (the green dot): online and sharing.
    pub online: bool,
    /// `macos`, `linux`, `windows`, or empty.
    pub os: String,
    /// The machine this app runs on.
    pub current: bool,
    /// Someone shares it with you (a team machine).
    pub team: bool,
    /// The page's status line ("Online", "Offline · last seen 9 days ago").
    pub status: String,
    /// After the status: who shares it ("Shared by your team").
    #[serde(default)]
    pub shared_by: Option<String>,
    /// Rename, when it can be renamed from here.
    #[serde(default)]
    pub rename: Option<MachinesViewRename>,
    /// One line over the switches (offline, signed out).
    #[serde(default)]
    pub notice: Option<String>,
    /// The notice's button (Sign In).
    #[serde(default)]
    pub notice_action: Option<crate::host::HostAction>,
    /// Provide Spaces, then Share this desktop.
    pub toggles: Vec<MachinesViewToggle>,
    /// Under read-only switches: where they change.
    #[serde(default)]
    pub toggles_note: Option<String>,
    /// "Spaces on this machine".
    pub spaces_title: String,
    /// The Spaces on it.
    pub spaces: Vec<MachinesViewSpace>,
    /// When it has none.
    #[serde(default)]
    pub spaces_empty: Option<String>,
    /// New Space here.
    pub new_space: MachinesViewNewSpace,
    /// "Details" (collapsed).
    pub details_title: String,
    /// OS, App, Connection, Service.
    pub details: Vec<MachinesViewFact>,
    /// Remove from account (none for a team machine).
    #[serde(default)]
    pub remove: Option<MachinesViewRemove>,
}

/// "Remove old machines": every machine of the group that can go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewRemoveAll {
    /// "Remove old machines".
    pub label: String,
    /// Asked first.
    pub confirm: HostConfirm,
    /// What each removal does.
    pub removes: Vec<MachinesViewRemove>,
}

/// A group of the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesViewGroup {
    /// `yours`, `old` or `team`.
    pub id: String,
    /// "Your machines", "Offline for 30+ days (2)", "Team machines".
    pub title: String,
    /// Collapsed until opened (the old machines).
    pub collapsed: bool,
    /// Its machines' ids, in order.
    pub ids: Vec<String>,
    /// Remove old machines.
    #[serde(default)]
    pub remove_all: Option<MachinesViewRemoveAll>,
}

/// The Machines page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MachinesView {
    /// "3 machines, 2 online".
    pub summary: String,
    /// The list's groups (only those with machines).
    pub groups: Vec<MachinesViewGroup>,
    /// Every machine, in list order.
    pub machines: Vec<MachinesViewMachine>,
}

/// "Your machines".
pub const YOURS_TITLE: &str = "Your machines";
/// "Team machines".
pub const TEAM_TITLE: &str = "Team machines";
/// The button that removes a machine.
pub const REMOVE_LABEL: &str = "Remove from account";
/// The button that opens New Space with the machine chosen.
pub const NEW_SPACE_LABEL: &str = "New Space here";
/// Under another machine's switches.
pub const OFFLINE_NOTICE: &str = "This machine is offline. Open Cua Spaces on it to use it.";

/// "just now", "5 minutes ago", "1 day ago", "9 days ago".
pub fn ago(then_secs: u64, now_secs: u64) -> String {
    let s = now_secs.saturating_sub(then_secs);
    let (n, unit) = match s {
        0..=59 => return "just now".into(),
        60..=3599 => (s / 60, "minute"),
        3600..=86_399 => (s / 3600, "hour"),
        _ => (s / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// "1 Space", "3 Spaces".
fn spaces_n(n: usize) -> String {
    format!("{n} Space{}", if n == 1 { "" } else { "s" })
}

/// "that Mac", "that PC", "that computer".
fn that_machine(os: &str) -> &'static str {
    match os {
        "macos" => "that Mac",
        "windows" => "that PC",
        _ => "that computer",
    }
}

/// The Spaces on a machine: this machine's local Spaces, or the ones
/// another machine provides. Never a machine's own desktop.
fn spaces_on<'a>(spaces: &'a [Space], row: &MachinePageRow) -> Vec<&'a Space> {
    if row.device_only {
        return vec![];
    }
    spaces
        .iter()
        .filter(|s| {
            if row.current {
                s.provider == Some(SpaceProvider::Local) && s.id != crate::host::THIS_MACHINE_ID
            } else {
                s.host.as_deref() == Some(row.id.as_str())
            }
        })
        .collect()
}

fn running(s: &Space) -> bool {
    matches!(s.status, SpaceStatus::Running)
}

fn space_os(s: &Space) -> String {
    s.os_pretty_name
        .clone()
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| match s.os {
            SpaceOs::Unknown => String::new(),
            os => os.label().into(),
        })
}

fn os_label(os: &str) -> String {
    SpaceOs::parse(os).map_or_else(String::new, |o| o.label().to_string())
}

/// The offline line: "Offline · last seen 9 days ago", or "Offline".
fn offline_line(last_seen: Option<u64>, now: u64) -> String {
    match last_seen {
        Some(t) if t > 0 => format!("{} \u{b7} last seen {}", machines::OFFLINE, ago(t, now)),
        _ => machines::OFFLINE.into(),
    }
}

/// What a machine's switches show and do.
struct Switches {
    provide: bool,
    share: bool,
}

/// This machine's switches, from its host state.
fn host_toggles(s: Option<&HostState>) -> Vec<MachinesViewToggle> {
    let provide_help = "Your devices can create Spaces here.";
    let share_help = "Your devices can see and control this screen.";
    let Some(s) = s.filter(|s| s.configured) else {
        // Before setup: either switch opens the setup form.
        let loading = s.is_none();
        return vec![
            MachinesViewToggle {
                id: "spaces".into(),
                label: "Provide Spaces".into(),
                help: provide_help.into(),
                on: false,
                editable: true,
                enabled: !loading,
                actions: vec![],
                setup: Some("spare".into()),
            },
            MachinesViewToggle {
                id: "desktop".into(),
                label: "Share this desktop".into(),
                help: share_help.into(),
                on: false,
                editable: true,
                enabled: !loading,
                actions: vec![],
                setup: Some("desktop".into()),
            },
        ];
    };
    let relay = s.mode.as_deref() != Some("direct");
    // Stopped sharing reads as both switches off: the switches are the one
    // control, so there is no separate "Not sharing" card to reconcile.
    let live = s.sharing || s.paused_signed_out;
    let provide_on = s.provide_spaces && live;
    let share_on = s.share_desktop && live;
    let flip = |on: bool,
                setting: bool,
                turn_on: HostActionId,
                turn_off: HostActionId,
                other_setting: bool,
                other_off: HostActionId| {
        if on {
            return vec![turn_off];
        }
        let mut a = vec![];
        if !setting {
            a.push(turn_on);
        }
        if !s.sharing && !s.paused_signed_out {
            // Resuming turns on every setting that is on: keep the other
            // one off, as its switch shows it.
            if other_setting {
                a.push(other_off);
            }
            a.push(HostActionId::ResumeSharing);
        }
        a
    };
    let enabled = relay && !s.paused_signed_out;
    let provide_help = if relay {
        provide_help
    } else {
        "Needs the relay."
    };
    vec![
        MachinesViewToggle {
            id: "spaces".into(),
            label: "Provide Spaces".into(),
            help: provide_help.into(),
            on: provide_on,
            editable: true,
            enabled,
            actions: flip(
                provide_on,
                s.provide_spaces,
                HostActionId::ProvideSpaces,
                HostActionId::StopProvidingSpaces,
                s.share_desktop,
                HostActionId::HideDesktop,
            ),
            setup: None,
        },
        MachinesViewToggle {
            id: "desktop".into(),
            label: "Share this desktop".into(),
            help: share_help.into(),
            on: share_on,
            editable: true,
            enabled,
            actions: flip(
                share_on,
                s.share_desktop,
                HostActionId::ShareDesktop,
                HostActionId::HideDesktop,
                s.provide_spaces,
                HostActionId::StopProvidingSpaces,
            ),
            setup: None,
        },
    ]
}

/// Another machine's switches: read-only, showing what it does.
fn remote_toggles(sw: &Switches, team: bool) -> Vec<MachinesViewToggle> {
    let t = |id: &str, label: &str, help: &str, on: bool| MachinesViewToggle {
        id: id.into(),
        label: label.into(),
        help: help.into(),
        on,
        editable: false,
        enabled: false,
        actions: vec![],
        setup: None,
    };
    let mut out = vec![t(
        "spaces",
        "Provide Spaces",
        "Your devices can create Spaces here.",
        sw.provide,
    )];
    if !team {
        out.push(t(
            "desktop",
            "Share this desktop",
            "Your devices can see and control this screen.",
            sw.share,
        ));
    }
    out
}

/// The confirmation before a machine leaves the account.
fn remove_confirm(name: &str, current: bool, device_only: bool) -> HostConfirm {
    let message = if current {
        "Everyone connected is disconnected, the host service is uninstalled and \
         this machine leaves your machines. Turning a switch on again sets it up again."
            .to_string()
    } else if device_only {
        format!("{name} will no longer list or reach your machines. Sign in on it again to use it.")
    } else {
        format!(
            "{name} leaves your machines, and its Spaces are no longer listed here. \
             Setting it up again on it adds it back."
        )
    };
    HostConfirm {
        title: format!("Remove \u{201c}{name}\u{201d} from your account?"),
        message,
        confirm_label: "Remove".into(),
        cancel_label: "Cancel".into(),
    }
}

/// The Machines page.
pub fn view(input: &MachinesViewInput) -> MachinesView {
    let rows = input
        .rows
        .clone()
        .unwrap_or_else(|| machines::page_rows(&input.page));
    let now = input.page.now;
    let spaces = &input.page.spaces;
    let host = input.host.as_ref();
    let mut list = Vec::with_capacity(rows.len());
    let mut yours = vec![];
    let mut old = vec![];
    let mut team_ids = vec![];
    let mut old_removes = vec![];
    for row in &rows {
        let relay = input.relay.iter().find(|r| r.id == row.id);
        let team = !row.current && relay.is_some_and(|r| !r.role.is_empty() && r.role != "owner");
        let on = spaces_on(spaces, row);
        let n_running = on.iter().filter(|s| running(s)).count();
        let own_space = row
            .space_id
            .as_deref()
            .and_then(|id| spaces.iter().find(|s| s.id == id));
        let sw = if row.current {
            // As its switches show them: stopped sharing is both off.
            let live = |s: &HostState| s.configured && (s.sharing || s.paused_signed_out);
            Switches {
                provide: host.is_some_and(|s| live(s) && s.provide_spaces),
                share: host.is_some_and(|s| live(s) && s.share_desktop),
            }
        } else {
            Switches {
                provide: !row.device_only
                    && relay
                        .and_then(|r| r.provides_spaces)
                        .unwrap_or(!on.is_empty()),
                share: own_space
                    .is_some_and(|s| crate::spaces::sidebar::shares_desktop(s) != Some(false)),
            }
        };
        let live = row.online && !row.stopped_sharing;
        let name = if row.current {
            host.and_then(|s| s.name.clone())
                .filter(|n| s_nonempty(n))
                .or_else(|| Some(input.about.computer_name.clone()).filter(|n| s_nonempty(n)))
                .unwrap_or_else(|| row.name.clone())
        } else {
            row.name.clone()
        };

        // The row's one line.
        let line = if !row.online {
            offline_line(row.last_seen, now)
        } else if row.stopped_sharing {
            format!("{} \u{b7} {}", machines::NOT_SHARING, row.spaces_label)
        } else if row.device_only {
            machines::ONLINE.into()
        } else {
            let mut parts = vec![];
            if n_running > 0 {
                parts.push(format!("{} running", spaces_n(n_running)));
            } else if !on.is_empty() {
                parts.push(spaces_n(on.len()));
            }
            if sw.provide {
                if parts.is_empty() {
                    parts.push("Ready for Spaces".into());
                } else {
                    parts.push("Provides Spaces".into());
                }
            }
            if parts.is_empty() {
                parts.push(machines::spaces_label(0));
            }
            parts.join(" \u{b7} ")
        };

        let status = if !row.online {
            offline_line(row.last_seen, now)
        } else if row.stopped_sharing {
            machines::NOT_SHARING.into()
        } else {
            machines::ONLINE.into()
        };
        let shared_by = team.then(|| {
            let r = relay.expect("a team machine has a relay row");
            if r.team.as_deref().is_some_and(s_nonempty) {
                "Shared by your team".to_string()
            } else {
                match r.owner_email.as_deref().filter(|e| s_nonempty(e)) {
                    Some(e) => format!("Shared by {e}"),
                    None => "Shared with you".into(),
                }
            }
        });

        let rename = if row.current {
            host.filter(|s| {
                s.configured && s.mode.as_deref() != Some("direct") && !s.paused_signed_out
            })
            .map(|_| ("host", row.id.clone()))
        } else if row.device_only {
            row.device.clone().map(|d| ("device", d))
        } else if relay.is_some_and(|r| r.role == "owner") {
            Some(("machine", row.id.clone()))
        } else {
            None
        }
        .map(|(target, id)| MachinesViewRename {
            target: target.into(),
            id,
            title: "Rename Machine".into(),
            confirm_label: "Rename".into(),
        });

        let (notice, notice_action) = if row.current {
            let p = crate::host::panel(host);
            (p.notice, p.notice_action)
        } else if !row.online {
            (Some(OFFLINE_NOTICE.into()), None)
        } else if row.stopped_sharing {
            (
                Some(format!(
                    "Sharing is off on {}. Turn it on there to use it.",
                    that_machine(&row.os)
                )),
                None,
            )
        } else {
            (None, None)
        };

        let toggles = if row.current {
            host_toggles(host)
        } else {
            remote_toggles(&sw, team)
        };
        let toggles_note = if row.current {
            None
        } else if team {
            Some(format!(
                "Its owner changes this on {}.",
                that_machine(&row.os)
            ))
        } else if live {
            Some(format!("Change this on {}.", that_machine(&row.os)))
        } else {
            None
        };

        let new_space = MachinesViewNewSpace {
            label: NEW_SPACE_LABEL.into(),
            on: if row.current {
                "local".into()
            } else {
                format!("host:{}", row.id)
            },
            enabled: row.current || (live && sw.provide),
            help: if row.current {
                None
            } else if !row.online {
                Some("It\u{2019}s offline.".into())
            } else if row.stopped_sharing {
                Some("It isn\u{2019}t sharing.".into())
            } else if !sw.provide {
                Some(format!(
                    "It doesn\u{2019}t provide Spaces. Turn on Provide Spaces on {}.",
                    that_machine(&row.os)
                ))
            } else {
                None
            },
        };

        let mut details = vec![];
        let mut fact = |label: &str, value: String| {
            if !value.trim().is_empty() {
                details.push(MachinesViewFact {
                    label: label.into(),
                    value,
                });
            }
        };
        if row.current {
            fact(
                "OS",
                if s_nonempty(&input.about.os_version) {
                    input.about.os_version.clone()
                } else {
                    os_label(&row.os)
                },
            );
            fact("App", input.about.app_version.clone());
            match host {
                Some(s) if s.configured => {
                    fact(
                        "Connection",
                        if s.mode.as_deref() == Some("direct") {
                            "Direct".into()
                        } else {
                            "via Cua relay".into()
                        },
                    );
                    fact(
                        "Service",
                        if s.service_running {
                            "Running"
                        } else if s.service_installed {
                            "Stopped"
                        } else {
                            "Not installed"
                        }
                        .into(),
                    );
                }
                Some(_) => fact("Connection", "Not shared".into()),
                None => {}
            }
        } else {
            fact(
                "OS",
                own_space
                    .and_then(|s| s.os_pretty_name.clone())
                    .filter(|n| s_nonempty(n))
                    .unwrap_or_else(|| os_label(&row.os)),
            );
            if !row.device_only {
                fact(
                    "Connection",
                    if row.via == "direct" {
                        "Direct".into()
                    } else {
                        "via Cua relay".into()
                    },
                );
                fact(
                    "Service",
                    if !row.online {
                        "Not reachable"
                    } else if row.stopped_sharing {
                        machines::NOT_SHARING
                    } else {
                        "Running"
                    }
                    .into(),
                );
            }
        }

        let remove = if team {
            None
        } else if row.current {
            host.filter(|s| s.configured).map(|_| MachinesViewRemove {
                label: REMOVE_LABEL.into(),
                enabled: true,
                help: None,
                host: true,
                machine: None,
                device: None,
                confirm: remove_confirm(&name, true, false),
            })
        } else if row.device_only {
            Some(MachinesViewRemove {
                label: REMOVE_LABEL.into(),
                enabled: true,
                help: None,
                host: false,
                machine: None,
                device: row.device.clone(),
                confirm: remove_confirm(&name, false, true),
            })
        } else {
            // The relay takes a machine off only while it is not
            // connected: one that is online would register again.
            Some(MachinesViewRemove {
                label: REMOVE_LABEL.into(),
                enabled: !row.online,
                help: row.online.then(|| {
                    format!(
                        "It\u{2019}s online. Remove it on {}, or here once it\u{2019}s offline.",
                        that_machine(&row.os)
                    )
                }),
                host: false,
                machine: Some(row.id.clone()),
                device: row.device.clone(),
                confirm: remove_confirm(&name, false, false),
            })
        };

        let is_old = !row.current
            && !team
            && !row.online
            && row
                .last_seen
                .is_some_and(|t| t > 0 && now.saturating_sub(t) >= OLD_AFTER_SECS);
        if team {
            team_ids.push(row.id.clone());
        } else if is_old {
            old.push(row.id.clone());
            if let Some(r) = remove.clone().filter(|r| r.enabled) {
                old_removes.push(r);
            }
        } else {
            yours.push(row.id.clone());
        }

        list.push(MachinesViewMachine {
            id: row.id.clone(),
            name,
            badge: row
                .current
                .then(|| input.page.host_os.this_machine().to_string()),
            line,
            online: live,
            os: row.os.clone(),
            current: row.current,
            team,
            status,
            shared_by,
            rename,
            notice,
            notice_action,
            toggles,
            toggles_note,
            spaces_title: "Spaces on this machine".into(),
            spaces_empty: on.is_empty().then(|| {
                if row.online {
                    "No Spaces yet.".into()
                } else {
                    "No Spaces. The machine is offline.".into()
                }
            }),
            spaces: on
                .iter()
                .map(|s| MachinesViewSpace {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    os: space_os(s),
                    status: crate::spaces::sidebar::status_text(s),
                    running: running(s),
                })
                .collect(),
            new_space,
            details_title: "Details".into(),
            details,
            remove,
        });
    }

    let mut groups = vec![];
    if !yours.is_empty() {
        groups.push(MachinesViewGroup {
            id: "yours".into(),
            title: YOURS_TITLE.into(),
            collapsed: false,
            ids: yours,
            remove_all: None,
        });
    }
    if !old.is_empty() {
        let n = old_removes.len();
        groups.push(MachinesViewGroup {
            id: "old".into(),
            title: format!("Offline for 30+ days ({})", old.len()),
            collapsed: true,
            ids: old,
            remove_all: (n > 0).then(|| MachinesViewRemoveAll {
                label: "Remove old machines".into(),
                confirm: HostConfirm {
                    title: if n == 1 {
                        "Remove 1 old machine?".into()
                    } else {
                        format!("Remove {n} old machines?")
                    },
                    message: "They have been offline for 30 days or more. They leave your \
                              account; setting one up again adds it back."
                        .into(),
                    confirm_label: "Remove".into(),
                    cancel_label: "Cancel".into(),
                },
                removes: old_removes,
            }),
        });
    }
    if !team_ids.is_empty() {
        groups.push(MachinesViewGroup {
            id: "team".into(),
            title: TEAM_TITLE.into(),
            collapsed: false,
            ids: team_ids,
            remove_all: None,
        });
    }
    // List order follows the groups.
    let order: Vec<&String> = groups.iter().flat_map(|g| g.ids.iter()).collect();
    list.sort_by_key(|m| {
        order
            .iter()
            .position(|id| **id == m.id)
            .unwrap_or(usize::MAX)
    });
    let total = list.len();
    let online = list.iter().filter(|m| m.online).count();
    MachinesView {
        summary: format!(
            "{total} machine{}, {online} online",
            if total == 1 { "" } else { "s" }
        ),
        groups,
        machines: list,
    }
}

fn s_nonempty(s: &str) -> bool {
    !s.trim().is_empty()
}

/// A new name as saved: trimmed, at most 64 characters (the relay's
/// limit); `None` when nothing is left.
pub fn clean_name(name: &str) -> Option<String> {
    crate::devices::clean_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machines::DeviceIn;

    const NOW: u64 = 1_800_000_000;
    const DAY: u64 = 86_400;

    fn space(v: serde_json::Value) -> Space {
        let mut base = serde_json::json!({
            "name": "", "os": "linux", "status": "running", "detail": "",
            "lastUsedAt": 0, "scene": "linux-terminal"
        });
        for (k, val) in v.as_object().unwrap() {
            base[k] = val.clone();
        }
        serde_json::from_value(base).unwrap()
    }

    fn host(provide: bool, share: bool) -> HostState {
        HostState {
            configured: true,
            mode: Some("relay".into()),
            name: Some("Dillon's MacBook Pro".into()),
            sharing: true,
            service_installed: true,
            service_running: true,
            service_kind: "launchd".into(),
            share_desktop: share,
            provide_spaces: provide,
            ..Default::default()
        }
    }

    fn device(id: &str, name: &str, seen: u64) -> DeviceIn {
        DeviceIn {
            id: id.into(),
            name: name.into(),
            platform: "macos".into(),
            last_seen: Some(seen),
            state: "enrolled".into(),
            ..Default::default()
        }
    }

    fn relay(id: &str, role: &str, provides: Option<bool>) -> MachinesViewRelay {
        MachinesViewRelay {
            id: id.into(),
            role: role.into(),
            provides_spaces: provides,
            ..Default::default()
        }
    }

    /// The approved mockup: This Mac with two Spaces, a Mac Studio that
    /// provides one, an offline Mac, two machines offline for weeks and a
    /// team machine.
    fn mockup() -> MachinesViewInput {
        MachinesViewInput {
            page: MachinesPageInput {
                spaces: vec![
                    space(
                        serde_json::json!({"id": "this-mac", "name": "This Mac", "os": "macos", "status": "local"}),
                    ),
                    space(
                        serde_json::json!({"id": "local:space-cdf71481c4", "name": "space-cdf71481c4", "os": "macos", "osPrettyName": "macOS 26.5.2", "provider": "local"}),
                    ),
                    space(
                        serde_json::json!({"id": "local:space-1cef393ac1", "name": "space-1cef393ac1", "provider": "local"}),
                    ),
                    space(serde_json::json!({
                        "id": "relay:studio", "name": "cua's Mac Studio", "os": "macos",
                        "osPrettyName": "macOS 26.5.2", "status": "running", "provider": "relay"
                    })),
                    space(serde_json::json!({
                        "id": "relay:studio/xm", "name": "xm-before-linux", "osPrettyName": "Ubuntu 24.04.5 LTS",
                        "provider": "relay", "host": "studio", "hostName": "cua's Mac Studio"
                    })),
                    space(
                        serde_json::json!({"id": "relay:byoc", "name": "Cua BYOC tests", "os": "macos", "status": "suspended", "provider": "relay"}),
                    ),
                    space(
                        serde_json::json!({"id": "relay:old1", "name": "mac-mini-staging", "os": "macos", "status": "suspended", "provider": "relay"}),
                    ),
                    space(
                        serde_json::json!({"id": "relay:team", "name": "build-mini-01", "os": "macos", "status": "running", "provider": "relay"}),
                    ),
                ],
                devices: vec![
                    device("dev_byoc", "Cua BYOC tests", NOW - 9 * DAY),
                    device("dev_old1", "mac-mini-staging", NOW - 41 * DAY),
                ],
                now: NOW,
                ..Default::default()
            },
            relay: vec![
                relay("studio", "owner", Some(true)),
                relay("byoc", "owner", Some(true)),
                relay("old1", "owner", Some(false)),
                MachinesViewRelay {
                    team: Some("team-1".into()),
                    ..relay("team", "shared", Some(true))
                },
            ],
            host: Some(host(false, true)),
            about: MachinesViewAbout {
                computer_name: "Dillon's MacBook Pro".into(),
                app_version: "0.9.1".into(),
                os_version: "macOS 26.5.2".into(),
            },
            rows: None,
        }
    }

    /// Rows an older host listed (merged by the web UI) draw the same page.
    #[test]
    fn rows_the_shell_listed_draw_the_same_page() {
        let input = mockup();
        let rows = machines::page_rows(&input.page);
        let given = view(&MachinesViewInput {
            rows: Some(rows),
            ..input.clone()
        });
        assert_eq!(given, view(&input));
    }

    fn by_id<'a>(v: &'a MachinesView, id: &str) -> &'a MachinesViewMachine {
        v.machines.iter().find(|m| m.id == id).unwrap()
    }

    #[test]
    fn the_list_groups_machines_with_one_line_each() {
        let v = view(&mockup());
        let groups: Vec<(&str, &str, bool, Vec<&str>)> = v
            .groups
            .iter()
            .map(|g| {
                (
                    g.id.as_str(),
                    g.title.as_str(),
                    g.collapsed,
                    g.ids.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            groups,
            [
                (
                    "yours",
                    "Your machines",
                    false,
                    vec!["this-mac", "studio", "byoc"]
                ),
                ("old", "Offline for 30+ days (1)", true, vec!["old1"]),
                ("team", "Team machines", false, vec!["team"]),
            ]
        );
        let lines: Vec<(&str, &str, Option<&str>)> = v
            .machines
            .iter()
            .map(|m| (m.name.as_str(), m.line.as_str(), m.badge.as_deref()))
            .collect();
        assert_eq!(
            lines,
            [
                ("Dillon's MacBook Pro", "2 Spaces running", Some("This Mac")),
                (
                    "cua's Mac Studio",
                    "1 Space running \u{b7} Provides Spaces",
                    None
                ),
                (
                    "Cua BYOC tests",
                    "Offline \u{b7} last seen 9 days ago",
                    None
                ),
                (
                    "mac-mini-staging",
                    "Offline \u{b7} last seen 41 days ago",
                    None
                ),
                ("build-mini-01", "Ready for Spaces", None),
            ]
        );
        assert_eq!(v.summary, "5 machines, 3 online");
        let all = v.groups[1].remove_all.as_ref().unwrap();
        assert_eq!(all.label, "Remove old machines");
        assert_eq!(all.removes[0].machine.as_deref(), Some("old1"));
        assert_eq!(all.removes[0].device.as_deref(), Some("dev_old1"));
    }

    #[test]
    fn only_this_machine_s_switches_change_here() {
        let v = view(&mockup());
        let me = by_id(&v, "this-mac");
        let t: Vec<(&str, bool, bool)> = me
            .toggles
            .iter()
            .map(|t| (t.label.as_str(), t.on, t.editable && t.enabled))
            .collect();
        assert_eq!(
            t,
            [
                ("Provide Spaces", false, true),
                ("Share this desktop", true, true)
            ]
        );
        assert_eq!(me.toggles[0].actions, [HostActionId::ProvideSpaces]);
        assert_eq!(me.toggles[1].actions, [HostActionId::HideDesktop]);
        assert_eq!(me.toggles_note, None);

        let studio = by_id(&v, "studio");
        assert!(studio.toggles.iter().all(|t| !t.editable && !t.enabled));
        assert_eq!(
            studio.toggles.iter().map(|t| t.on).collect::<Vec<_>>(),
            [true, true]
        );
        assert_eq!(
            studio.toggles_note.as_deref(),
            Some("Change this on that Mac.")
        );

        let team = by_id(&v, "team");
        assert_eq!(team.toggles.len(), 1);
        assert_eq!(
            team.toggles_note.as_deref(),
            Some("Its owner changes this on that Mac.")
        );
        assert_eq!(team.shared_by.as_deref(), Some("Shared by your team"));
        assert!(team.remove.is_none() && team.rename.is_none());

        let byoc = by_id(&v, "byoc");
        assert_eq!(byoc.notice.as_deref(), Some(OFFLINE_NOTICE));
        assert_eq!(byoc.toggles_note, None);
        assert_eq!(byoc.status, "Offline \u{b7} last seen 9 days ago");
    }

    #[test]
    fn stopped_sharing_reads_as_switches_off_and_turning_one_on_resumes() {
        let mut input = mockup();
        input.host = Some(HostState {
            sharing: false,
            ..host(true, true)
        });
        let v = view(&input);
        let me = by_id(&v, "this-mac");
        assert!(me.toggles.iter().all(|t| !t.on && t.enabled));
        // Turning Provide Spaces on resumes sharing, keeping the desktop off
        // as its switch shows.
        assert_eq!(
            me.toggles[0].actions,
            [HostActionId::HideDesktop, HostActionId::ResumeSharing]
        );
        // No "Not sharing" card: the switches are the one control.
        assert_eq!(me.notice, None);
    }

    #[test]
    fn before_setup_a_switch_opens_the_setup_form() {
        let mut input = mockup();
        input.host = Some(HostState::default());
        let v = view(&input);
        let me = by_id(&v, "this-mac");
        assert_eq!(
            me.toggles
                .iter()
                .map(|t| t.setup.as_deref())
                .collect::<Vec<_>>(),
            [Some("spare"), Some("desktop")]
        );
        assert!(me.remove.is_none() && me.rename.is_none());
        assert_eq!(me.name, "Dillon's MacBook Pro");
    }

    #[test]
    fn space_rows_link_to_the_space_and_new_space_here_chooses_the_machine() {
        let v = view(&mockup());
        let me = by_id(&v, "this-mac");
        assert_eq!(
            me.spaces
                .iter()
                .map(|s| (s.id.as_str(), s.os.as_str(), s.status.as_str()))
                .collect::<Vec<_>>(),
            [
                ("local:space-cdf71481c4", "macOS 26.5.2", "Running"),
                ("local:space-1cef393ac1", "Linux", "Running"),
            ]
        );
        assert_eq!(me.new_space.on, "local");
        assert!(me.new_space.enabled);
        let studio = by_id(&v, "studio");
        assert_eq!(studio.spaces[0].id, "relay:studio/xm");
        assert_eq!(studio.spaces[0].os, "Ubuntu 24.04.5 LTS");
        assert_eq!(studio.new_space.on, "host:studio");
        assert!(studio.new_space.enabled);
        let byoc = by_id(&v, "byoc");
        assert!(!byoc.new_space.enabled);
        assert_eq!(
            byoc.spaces_empty.as_deref(),
            Some("No Spaces. The machine is offline.")
        );
        assert!(by_id(&v, "team").new_space.enabled);
    }

    #[test]
    fn details_and_remove_from_account() {
        let v = view(&mockup());
        let facts = |id: &str| -> Vec<(String, String)> {
            by_id(&v, id)
                .details
                .iter()
                .map(|f| (f.label.clone(), f.value.clone()))
                .collect()
        };
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(
            facts("this-mac"),
            [
                pair("OS", "macOS 26.5.2"),
                pair("App", "0.9.1"),
                pair("Connection", "via Cua relay"),
                pair("Service", "Running"),
            ]
        );
        assert_eq!(
            facts("byoc"),
            [
                pair("OS", "macOS"),
                pair("Connection", "via Cua relay"),
                pair("Service", "Not reachable"),
            ]
        );
        let me = by_id(&v, "this-mac").remove.clone().unwrap();
        assert!(me.host && me.enabled);
        assert_eq!(me.label, "Remove from account");
        assert_eq!(
            me.confirm.title,
            "Remove \u{201c}Dillon's MacBook Pro\u{201d} from your account?"
        );
        // Another machine leaves the relay while it is offline; online, it
        // says where to remove it.
        let byoc = by_id(&v, "byoc").remove.clone().unwrap();
        assert!(byoc.enabled && !byoc.host);
        assert_eq!(byoc.machine.as_deref(), Some("byoc"));
        assert_eq!(byoc.device.as_deref(), Some("dev_byoc"));
        let studio = by_id(&v, "studio").remove.clone().unwrap();
        assert!(!studio.enabled);
        assert!(studio.help.unwrap().contains("that Mac"));
        // Rename saves on the host, the relay machine or the device.
        assert_eq!(
            by_id(&v, "this-mac")
                .rename
                .as_ref()
                .map(|r| r.target.as_str()),
            Some("host")
        );
        assert_eq!(
            by_id(&v, "studio")
                .rename
                .as_ref()
                .map(|r| r.target.as_str()),
            Some("machine")
        );
    }

    /// CUA-1617's machine: online, its owner stopped sharing it.
    #[test]
    fn a_machine_that_stopped_sharing_reads_not_sharing_no_spaces() {
        let input = MachinesViewInput {
            page: MachinesPageInput {
                spaces: vec![space(serde_json::json!({
                    "id": "relay:b695", "name": "Dillons-MacBook-Pro-2.local", "os": "macos",
                    "status": "suspended", "provider": "relay",
                    "sdk": {"features": [], "spacesdVersion": "", "reachable": false,
                            "error": "unauthenticated: Dillons-MacBook-Pro-2.local stopped sharing: ask its owner"}
                }))],
                now: NOW,
                ..Default::default()
            },
            ..Default::default()
        };
        let v = view(&input);
        let m = by_id(&v, "b695");
        assert_eq!(m.line, "Not sharing \u{b7} No Spaces");
        assert_eq!(m.status, "Not sharing");
        assert!(!m.online);
        assert!(!m.new_space.enabled);
    }

    #[test]
    fn ago_reads_plainly() {
        assert_eq!(ago(NOW - 30, NOW), "just now");
        assert_eq!(ago(NOW - 60, NOW), "1 minute ago");
        assert_eq!(ago(NOW - 2 * 3600, NOW), "2 hours ago");
        assert_eq!(ago(NOW - DAY, NOW), "1 day ago");
    }
}
