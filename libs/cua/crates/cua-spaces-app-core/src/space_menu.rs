// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A Space's "…" and right-click menu on the Spaces page (the same items in
//! both places, on a tile and on a list row).
//!
//! The menu shows only what you may do with the Space:
//!
//! * **Yours** (shared or not): Open, Turn on/off, Share… (Manage sharing…
//!   once it is shared with anyone), Teleport an app…, Rename, then Delete
//!   Space in red, set apart.
//! * **Shared with you**: Open, then Turn on/off and Teleport an app only
//!   when its owner allows them, then Leave, set apart (not red: nothing is
//!   deleted).
//! * **A team's Space**: Open, Turn on/off and Teleport an app when the team
//!   allows them, then Leave; Delete Space in place of Leave for the person
//!   who made it.
//!
//! Teleport an app… stays in the menu while the Space is off, greyed: it
//! needs a running desktop.
//!
//! Pin to notch / Unpin from notch is in every menu that can pin (a Space,
//! and the desktop of one of your other machines), whatever your role: it
//! keeps a tile in the notch first in line and grants nothing. It comes
//! after the Space's own actions, above the set-apart Delete or Leave.
//!
//! The input is its own plain record (the page builds it from the Space's
//! sharing facts), so the rule does not depend on how a shell keeps them.

use serde::{Deserialize, Serialize};

/// Whose Space it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpaceMenuKind {
    /// You own it (shared or not).
    #[default]
    Mine,
    /// Someone shared it with you.
    WithMe,
    /// It belongs to a team you are in.
    Team,
}

/// What you may do with it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpaceMenuRole {
    /// You own it.
    #[default]
    Owner,
    /// You may use it.
    Editor,
    /// You may only watch it: never power, teleport or rename, whatever
    /// the other flags say.
    Viewer,
}

/// Whether it runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpaceMenuPower {
    /// Running.
    Running,
    /// Stopped.
    #[default]
    Off,
    /// Turning on or off right now: the power item waits.
    Busy,
}

/// The facts the menu reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SpaceMenuInput {
    /// Whose Space it is.
    pub kind: SpaceMenuKind,
    /// Your role on it.
    pub role: SpaceMenuRole,
    /// A team's Space: you made it (Delete Space in place of Leave).
    pub maker: bool,
    /// Whether it runs.
    pub power: SpaceMenuPower,
    /// Yours and shared with at least one person (Manage sharing…).
    pub shared: bool,
    /// You may turn it on and off.
    pub can_power: bool,
    /// You may teleport an app into it.
    pub can_teleport: bool,
    /// You may share it.
    pub can_share: bool,
    /// You may rename it.
    pub can_rename: bool,
    /// You may delete it.
    pub can_delete: bool,
    /// You may leave it.
    pub can_leave: bool,
    /// It can be pinned to the notch (not This machine).
    pub can_pin: bool,
    /// It is pinned to the notch now (Unpin from notch).
    pub pinned: bool,
}

/// What an item does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SpaceMenuItemId {
    /// Opens the Space.
    Open,
    /// Turns it on or off (the label says which).
    Power,
    /// Opens the Share sheet (not shared yet).
    Share,
    /// Opens the Share sheet (shared already).
    ManageSharing,
    /// Teleports an app into it.
    Teleport,
    /// Renames it.
    Rename,
    /// Deletes it (asks first: `spaces.bulkConfirm`).
    Delete,
    /// Leaves it (asks first: `spaces.bulkConfirm`).
    Leave,
    /// Pins it to the notch, or unpins it (the label says which).
    Pin,
}

/// One menu item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceMenuItem {
    /// What it does.
    pub id: SpaceMenuItemId,
    /// "Open", "Turn off", "Manage sharing…", "Delete Space".
    pub label: String,
    /// It can be chosen (greyed when not).
    pub enabled: bool,
    /// Drawn in red (Delete Space only).
    pub destructive: bool,
    /// A separator goes above it.
    pub separator_before: bool,
}

fn item(id: SpaceMenuItemId, label: &str) -> SpaceMenuItem {
    SpaceMenuItem {
        id,
        label: label.into(),
        enabled: true,
        destructive: false,
        separator_before: false,
    }
}

/// The menu for one Space.
pub fn menu(input: &SpaceMenuInput) -> Vec<SpaceMenuItem> {
    use SpaceMenuItemId::*;
    let viewer = input.role == SpaceMenuRole::Viewer;
    let running = input.power == SpaceMenuPower::Running;
    let mut items = vec![item(Open, "Open")];
    if input.can_power && !viewer {
        let mut p = item(
            Power,
            if input.power == SpaceMenuPower::Off {
                "Turn on"
            } else {
                "Turn off"
            },
        );
        p.enabled = input.power != SpaceMenuPower::Busy;
        items.push(p);
    }
    let mine = input.kind == SpaceMenuKind::Mine;
    if mine && input.can_share {
        items.push(if input.shared {
            item(ManageSharing, "Manage sharing\u{2026}")
        } else {
            item(Share, "Share\u{2026}")
        });
    }
    if input.can_teleport && !viewer {
        let mut t = item(Teleport, "Teleport an app\u{2026}");
        t.enabled = running;
        items.push(t);
    }
    if mine && input.can_rename && !viewer {
        items.push(item(Rename, "Rename"));
    }
    if input.can_pin {
        items.push(item(
            Pin,
            if input.pinned {
                "Unpin from notch"
            } else {
                "Pin to notch"
            },
        ));
    }
    let delete = match input.kind {
        SpaceMenuKind::Mine => input.can_delete,
        SpaceMenuKind::Team => input.maker && input.can_delete,
        SpaceMenuKind::WithMe => false,
    };
    if delete {
        let mut d = item(Delete, "Delete Space");
        d.destructive = true;
        d.separator_before = true;
        items.push(d);
    } else if !mine && input.can_leave {
        let mut l = item(Leave, "Leave");
        l.separator_before = true;
        items.push(l);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mine() -> SpaceMenuInput {
        SpaceMenuInput {
            kind: SpaceMenuKind::Mine,
            role: SpaceMenuRole::Owner,
            power: SpaceMenuPower::Running,
            can_power: true,
            can_teleport: true,
            can_share: true,
            can_rename: true,
            can_delete: true,
            ..Default::default()
        }
    }

    fn lines(items: &[SpaceMenuItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| {
                format!(
                    "{}{}{}{}",
                    if i.separator_before { "--- " } else { "" },
                    i.label,
                    if i.enabled { "" } else { " (disabled)" },
                    if i.destructive { " (red)" } else { "" }
                )
            })
            .collect()
    }

    #[test]
    fn yours_running_and_not_shared() {
        assert_eq!(
            lines(&menu(&mine())),
            [
                "Open",
                "Turn off",
                "Share\u{2026}",
                "Teleport an app\u{2026}",
                "Rename",
                "--- Delete Space (red)"
            ]
        );
    }

    #[test]
    fn yours_shared_says_manage_sharing() {
        let m = menu(&SpaceMenuInput {
            shared: true,
            ..mine()
        });
        assert_eq!(m[2].id, SpaceMenuItemId::ManageSharing);
        assert_eq!(m[2].label, "Manage sharing\u{2026}");
    }

    #[test]
    fn stopped_turns_on_and_greys_teleport() {
        let m = menu(&SpaceMenuInput {
            power: SpaceMenuPower::Off,
            ..mine()
        });
        assert_eq!(
            lines(&m),
            [
                "Open",
                "Turn on",
                "Share\u{2026}",
                "Teleport an app\u{2026} (disabled)",
                "Rename",
                "--- Delete Space (red)"
            ]
        );
    }

    #[test]
    fn busy_waits_on_power_and_teleport() {
        let m = menu(&SpaceMenuInput {
            power: SpaceMenuPower::Busy,
            ..mine()
        });
        assert_eq!(lines(&m)[1], "Turn off (disabled)");
        assert_eq!(lines(&m)[3], "Teleport an app\u{2026} (disabled)");
    }

    #[test]
    fn shared_with_me_shows_only_what_the_owner_allows_and_leave() {
        let base = SpaceMenuInput {
            kind: SpaceMenuKind::WithMe,
            role: SpaceMenuRole::Editor,
            power: SpaceMenuPower::Off,
            can_power: true,
            can_leave: true,
            // Never shown for someone else's Space, whatever it says.
            can_share: true,
            can_rename: true,
            can_delete: true,
            ..Default::default()
        };
        assert_eq!(lines(&menu(&base)), ["Open", "Turn on", "--- Leave"]);
        let nothing = SpaceMenuInput {
            can_power: false,
            ..base.clone()
        };
        assert_eq!(lines(&menu(&nothing)), ["Open", "--- Leave"]);
        let teleport = SpaceMenuInput {
            can_teleport: true,
            power: SpaceMenuPower::Running,
            ..base
        };
        assert_eq!(
            lines(&menu(&teleport)),
            ["Open", "Turn off", "Teleport an app\u{2026}", "--- Leave"]
        );
    }

    #[test]
    fn leave_is_set_apart_but_not_red() {
        let m = menu(&SpaceMenuInput {
            kind: SpaceMenuKind::WithMe,
            can_leave: true,
            ..Default::default()
        });
        let leave = m.last().unwrap();
        assert_eq!(leave.id, SpaceMenuItemId::Leave);
        assert!(leave.separator_before);
        assert!(!leave.destructive);
    }

    #[test]
    fn a_team_space_greys_teleport_while_off_and_leaves() {
        let team = SpaceMenuInput {
            kind: SpaceMenuKind::Team,
            role: SpaceMenuRole::Editor,
            power: SpaceMenuPower::Off,
            can_power: true,
            can_teleport: true,
            can_leave: true,
            ..Default::default()
        };
        assert_eq!(
            lines(&menu(&team)),
            [
                "Open",
                "Turn on",
                "Teleport an app\u{2026} (disabled)",
                "--- Leave"
            ]
        );
        // Its maker deletes it instead.
        let maker = SpaceMenuInput {
            maker: true,
            can_delete: true,
            ..team.clone()
        };
        assert_eq!(
            lines(&menu(&maker)).last().unwrap(),
            "--- Delete Space (red)"
        );
        assert!(!menu(&maker).iter().any(|i| i.id == SpaceMenuItemId::Leave));
        // Delete needs the right too.
        let no_right = SpaceMenuInput {
            maker: true,
            ..team
        };
        assert_eq!(lines(&menu(&no_right)).last().unwrap(), "--- Leave");
    }

    #[test]
    fn a_viewer_never_powers_teleports_or_renames() {
        let m = menu(&SpaceMenuInput {
            kind: SpaceMenuKind::WithMe,
            role: SpaceMenuRole::Viewer,
            power: SpaceMenuPower::Running,
            can_power: true,
            can_teleport: true,
            can_leave: true,
            ..Default::default()
        });
        assert_eq!(lines(&m), ["Open", "--- Leave"]);
    }

    #[test]
    fn pin_to_notch_is_in_every_menu_that_can_pin_above_delete_and_leave() {
        let pin = |input: SpaceMenuInput| lines(&menu(&input));
        assert_eq!(
            pin(SpaceMenuInput {
                can_pin: true,
                ..mine()
            }),
            [
                "Open",
                "Turn off",
                "Share\u{2026}",
                "Teleport an app\u{2026}",
                "Rename",
                "Pin to notch",
                "--- Delete Space (red)"
            ]
        );
        assert_eq!(
            pin(SpaceMenuInput {
                can_pin: true,
                pinned: true,
                ..mine()
            })[5],
            "Unpin from notch"
        );
        // Someone else's Space, even a watch-only one: it grants nothing.
        let viewer = SpaceMenuInput {
            kind: SpaceMenuKind::WithMe,
            role: SpaceMenuRole::Viewer,
            can_leave: true,
            can_pin: true,
            ..Default::default()
        };
        assert_eq!(pin(viewer), ["Open", "Pin to notch", "--- Leave"]);
        // One of your other machines' desktops: not a Space to power, share
        // or delete, so just Open and the pin.
        let desktop = SpaceMenuInput {
            can_pin: true,
            ..Default::default()
        };
        assert_eq!(pin(desktop), ["Open", "Pin to notch"]);
        // Nothing offers it where it cannot pin (This machine).
        assert_eq!(pin(SpaceMenuInput::default()), ["Open"]);
    }

    #[test]
    fn ids_and_words_on_the_wire() {
        let v = serde_json::to_value(menu(&SpaceMenuInput {
            shared: true,
            ..mine()
        }))
        .unwrap();
        assert_eq!(v[2]["id"], "manageSharing");
        assert_eq!(v[5]["separatorBefore"], true);
        let input: SpaceMenuInput = serde_json::from_value(serde_json::json!({
            "kind": "withMe", "power": "busy", "canLeave": true
        }))
        .unwrap();
        assert_eq!(input.kind, SpaceMenuKind::WithMe);
        assert_eq!(input.power, SpaceMenuPower::Busy);
    }
}
