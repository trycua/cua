// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The main window's chrome and the menu bar item's menu: the words and
//! order both shells draw around the sidebar ([`crate::spaces::sidebar`])
//! and the Keyvault ([`crate::keyvault::browse`]).
//!
//! The sidebar reads, top to bottom: New Space and search, This machine,
//! one section per location, the Keyvault (its switch in the header, then
//! its categories and sites), and the account line with Sign in and
//! Settings at the foot.

use serde::{Deserialize, Serialize};

/// What the chrome depends on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChromeInput {
    /// The signed-in account.
    pub identity: Option<String>,
    /// Cua Cloud works (signed in, or client credentials).
    pub cloud_configured: bool,
    /// Signing in is possible here (the native shell).
    pub can_sign_in: bool,
    /// Settings, Experiments: the Volume page only with Cua Volume on
    /// (none: every page, for callers that predate experiments).
    pub experiments: Option<crate::experiments::Experiments>,
}

/// The main window's fixed words and the account line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MainChrome {
    /// Window title when nothing is selected.
    pub title: String,
    /// "New Space".
    pub new_space_label: String,
    /// "⌘N".
    pub new_space_shortcut: String,
    /// The sidebar search's placeholder.
    pub search_placeholder: String,
    /// The Keyvault section's title.
    pub keyvault_title: String,
    /// The account line: the identity, "API key" or "Not signed in".
    pub account: String,
    /// "Sign in", when signing in is offered.
    pub sign_in_label: Option<String>,
    /// "Settings".
    pub settings_label: String,
    /// "⌘,".
    pub settings_shortcut: String,
    /// With no Spaces: the heading.
    pub empty_title: String,
    /// With no Spaces: the button.
    pub empty_action: String,
    /// The sidebar's Volume page entry ("Volume"); none while the Cua Volume
    /// experiment is off (the page and its route are hidden; a mounted
    /// volume stays mounted).
    #[serde(default)]
    pub volume_label: Option<String>,
}

/// The chrome.
pub fn chrome(input: &ChromeInput) -> MainChrome {
    let identity = input.identity.clone().filter(|i| !i.is_empty());
    MainChrome {
        title: "Cua Spaces".into(),
        new_space_label: "New Space".into(),
        new_space_shortcut: "\u{2318}N".into(),
        search_placeholder: "Search".into(),
        keyvault_title: "Keyvault".into(),
        sign_in_label: (identity.is_none() && input.can_sign_in).then(|| "Sign in".into()),
        account: identity.unwrap_or_else(|| {
            if input.cloud_configured {
                "API key".into()
            } else {
                "Not signed in".into()
            }
        }),
        settings_label: "Settings".into(),
        settings_shortcut: "\u{2318},".into(),
        empty_title: "No Spaces yet".into(),
        empty_action: "New Space".into(),
        volume_label: volume_shown(input.experiments.as_ref()).then(|| "Volume".into()),
    }
}

/// Whether Cua Volume shows (its page, the menu's sync line): with its
/// experiment on, or when the caller passes no experiments.
pub fn volume_shown(experiments: Option<&crate::experiments::Experiments>) -> bool {
    experiments.is_none_or(|x| x.cua_volume)
}

/// What a menu item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MenuItemId {
    /// The status line (disabled).
    Status,
    /// A separator.
    Separator,
    /// Show the main window.
    Open,
    /// The New Space sheet.
    NewSpace,
    /// Settings.
    Settings,
    /// Quit.
    Quit,
    /// Cua Volume's conflicts: opens the Volume page.
    VolumeConflicts,
}

/// One item of the menu bar item's menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuItem {
    /// What it does.
    pub id: MenuItemId,
    /// Label (empty for a separator).
    pub label: String,
    /// "⌘,", "⌘Q".
    pub shortcut: Option<String>,
    /// Enabled.
    pub enabled: bool,
}

/// The menu bar item's menu for `spaces` Spaces.
pub fn menu_bar(spaces: u32) -> Vec<MenuItem> {
    menu_bar_with_keyvault(spaces, None)
}

/// The menu with, when Keyvault sign-ins are live in a Space, a second
/// status line saying so (the core's
/// [`crate::keyvault::view::sharing_label`]): the menu bar's signal when
/// the notch is hidden.
pub fn menu_bar_with_keyvault(spaces: u32, keyvault: Option<&str>) -> Vec<MenuItem> {
    let item = |id, label: &str, shortcut: Option<&str>, enabled| MenuItem {
        id,
        label: label.into(),
        shortcut: shortcut.map(str::to_string),
        enabled,
    };
    let mut items = vec![item(
        MenuItemId::Status,
        &crate::spaces::status_line(spaces),
        None,
        false,
    )];
    if let Some(label) = keyvault {
        items.push(item(MenuItemId::Status, label, None, false));
    }
    items.extend([
        item(MenuItemId::Separator, "", None, false),
        item(MenuItemId::Open, "Open Cua Spaces", None, true),
        item(MenuItemId::NewSpace, "New Space\u{2026}", None, true),
        item(
            MenuItemId::Settings,
            "Settings\u{2026}",
            Some("\u{2318},"),
            true,
        ),
        item(MenuItemId::Separator, "", None, false),
        item(MenuItemId::Quit, "Quit Cua Spaces", Some("\u{2318}Q"), true),
    ]);
    items
}

/// What the menu bar item's menu shows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MenuInput {
    /// The roster (with "This machine").
    pub spaces: Vec<crate::model::Space>,
    /// Live Keyvault sign-ins ([`crate::keyvault::view::sharing_label`]).
    pub keyvault: Option<String>,
    /// Cua Volume's `volume_sync_status`, when read.
    pub sync: Option<crate::drive_page::DriveSyncInput>,
    /// Now (Unix ms): a feed that has not answered for [`OFFLINE_AFTER_MS`]
    /// is offline.
    pub now_ms: u64,
    /// `volume_storage`'s backend (`fs`, `s3`), when read: this machine's
    /// store with no other device has nothing to sync.
    pub backend: Option<String>,
    /// Settings, Experiments: Cua Volume's sync state and conflicts only
    /// with Cua Volume on (none: shown, for callers that predate
    /// experiments). Off hides them; the volume keeps syncing.
    pub experiments: Option<crate::experiments::Experiments>,
}

/// No successful poll of the bucket for this long: Cua Volume is offline
/// (the feed polls at least every 5 s).
pub const OFFLINE_AFTER_MS: u64 = 30_000;

/// Cua Volume's sync state for the status line ("Synced", "Syncing 2\u{2026}",
/// "Offline"); none where sync does not apply (this machine's store, one
/// device).
pub fn volume_sync_word(sync: &crate::drive_page::DriveSyncInput, now_ms: u64) -> Option<String> {
    let stale = now_ms > 0
        && sync.last_poll_ms > 0
        && now_ms.saturating_sub(sync.last_poll_ms) > OFFLINE_AFTER_MS;
    match sync.feed.as_str() {
        "off" => None,
        "error" => Some("Offline".into()),
        _ if stale => Some("Offline".into()),
        _ if sync.pending_uploads > 0 => Some(format!("Syncing {}\u{2026}", sync.pending_uploads)),
        _ => Some("Synced".into()),
    }
}

/// The menu for `input`: the Spaces the user can open (the notch's count)
/// with Cua Volume's sync state next to it, its conflicts (they open the
/// Volume page), then the actions.
pub fn menu(input: &MenuInput) -> Vec<MenuItem> {
    let n = crate::spaces::openable_count(&input.spaces);
    let alone = |s: &&crate::drive_page::DriveSyncInput| {
        input.backend.as_deref() == Some("fs")
            && !s
                .devices
                .iter()
                .any(|d| !d.this_device && d.id != s.device_id)
    };
    let sync = input
        .sync
        .as_ref()
        .filter(|s| !alone(s) && volume_shown(input.experiments.as_ref()));
    let mut items = menu_bar_with_keyvault(n, input.keyvault.as_deref());
    if let Some(word) = sync.and_then(|s| volume_sync_word(s, input.now_ms)) {
        items[0].label = format!("{} \u{b7} {word}", items[0].label);
    }
    let conflicts = sync
        .filter(|s| s.feed != "off")
        .map_or(0, |s| s.conflicts.len());
    if conflicts > 0 {
        let at = 1 + usize::from(input.keyvault.is_some());
        items.insert(
            at,
            MenuItem {
                id: MenuItemId::VolumeConflicts,
                label: if conflicts == 1 {
                    "1 conflict".into()
                } else {
                    format!("{conflicts} conflicts")
                },
                shortcut: None,
                enabled: true,
            },
        );
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_account_line_prefers_the_identity() {
        let c = chrome(&ChromeInput {
            identity: Some("ada@example.com".into()),
            cloud_configured: true,
            can_sign_in: true,
            experiments: None,
        });
        assert_eq!(c.account, "ada@example.com");
        assert_eq!(c.sign_in_label, None);
        let c = chrome(&ChromeInput {
            can_sign_in: true,
            ..Default::default()
        });
        assert_eq!(c.account, "Not signed in");
        assert_eq!(c.sign_in_label.as_deref(), Some("Sign in"));
        assert_eq!(
            chrome(&ChromeInput {
                cloud_configured: true,
                ..Default::default()
            })
            .account,
            "API key"
        );
    }

    /// Cua Volume off: no Volume page in the sidebar, no sync word or
    /// conflicts in the menu; on (or no experiments given): both.
    #[test]
    fn the_volume_experiment_decides_the_volume_page_and_the_sync_line() {
        use crate::experiments::Experiments;
        let off = Some(Experiments::default());
        let on = Some(Experiments {
            cua_volume: true,
            ..Default::default()
        });
        let chrome_with = |experiments| {
            chrome(&ChromeInput {
                experiments,
                ..Default::default()
            })
            .volume_label
        };
        assert_eq!(chrome_with(off), None);
        assert_eq!(chrome_with(on).as_deref(), Some("Volume"));
        assert_eq!(chrome_with(None).as_deref(), Some("Volume"));
        let sync: crate::drive_page::DriveSyncInput = serde_json::from_value(serde_json::json!({
            "feed": "live",
            "device_id": "dev-a",
            "pending_uploads": 2,
            "conflicts": [{"path": "/notes.md"}],
        }))
        .unwrap();
        let menu_with = |experiments| {
            menu(&MenuInput {
                sync: Some(sync.clone()),
                backend: Some("s3".into()),
                experiments,
                ..Default::default()
            })
        };
        let hidden = menu_with(off);
        assert!(!hidden[0].label.contains('\u{b7}'), "{}", hidden[0].label);
        assert!(hidden.iter().all(|i| i.id != MenuItemId::VolumeConflicts));
        let shown = menu_with(on);
        assert!(
            shown[0].label.ends_with("Syncing 2\u{2026}"),
            "{}",
            shown[0].label
        );
        assert!(shown.iter().any(|i| i.id == MenuItemId::VolumeConflicts));
        assert_eq!(menu_with(None), shown);
    }

    #[test]
    fn the_menu_counts_spaces() {
        let m = menu_bar(3);
        assert_eq!(m[0].label, "3 Spaces");
        assert!(!m[0].enabled);
        assert_eq!(m.last().unwrap().id, MenuItemId::Quit);
    }

    #[test]
    fn live_keyvault_sharing_shows_in_the_menu() {
        assert_eq!(menu_bar_with_keyvault(3, None), menu_bar(3));
        let m = menu_bar_with_keyvault(3, Some("Keyvault sign-ins live in dev-1"));
        assert_eq!(m.len(), menu_bar(3).len() + 1);
        assert_eq!(m[1].id, MenuItemId::Status);
        assert_eq!(m[1].label, "Keyvault sign-ins live in dev-1");
        assert!(!m[1].enabled);
        assert_eq!(m[2].id, MenuItemId::Separator);
    }

    #[test]
    fn the_menu_and_the_notch_count_the_same_spaces() {
        use crate::host::{HostSummaryInput, this_machine_space};
        use crate::model::{SpaceOs, SpaceStatus};
        let mut remote = this_machine_space(None, 0, SpaceOs::Linux);
        remote.id = "relay:studio".into();
        remote.name = "Studio".into();
        remote.status = SpaceStatus::Running;
        let hosting = HostSummaryInput {
            configured: true,
            service_running: true,
            sharing: true,
            ..Default::default()
        };
        for (host, extra, want) in [
            (None, vec![], 0),
            (Some(hosting.clone()), vec![], 1),
            (None, vec![remote.clone()], 1),
            (Some(hosting), vec![remote], 2),
        ] {
            let mut spaces = vec![this_machine_space(host.as_ref(), 0, SpaceOs::Macos)];
            spaces.extend(extra);
            let tab = crate::notch::tab(&spaces);
            let items = menu(&MenuInput {
                spaces: spaces.clone(),
                ..Default::default()
            });
            assert_eq!(tab.count, want.to_string());
            assert_eq!(items[0].label, crate::spaces::status_line(want));
        }
    }
}
