// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Launch at login, like Tailscale's "Launch Tailscale at login": the
//! Settings toggle (General), the first run's Done checkbox and the rule
//! for when the app turns it on by itself.
//!
//! The operating system holds the truth. Each shell reads it and passes
//! the status in ([`LoginItemInput`]); the toggle shows what the system
//! reports, never what was asked. The SwiftUI app asks `SMAppService` (the
//! app itself as a login item); the Tauri app writes an XDG autostart entry
//! on Linux, the `Run` key on Windows and a LaunchAgent on macOS.
//!
//! The app's own daemon starts with the app, so launching at login brings
//! the daemon back after a restart, and with it the Spaces this machine
//! provides, its persistent agents and Cua Volume.
//!
//! The default: on once the first run finishes (the Done checkbox, ticked
//! unless unticked). An install that finished its first run before this
//! setting existed is turned on only when it provides Spaces or runs
//! persistent agents ([`launch_plan`]). Once the user has chosen, the app
//! never changes it again.

use serde::{Deserialize, Serialize};

use crate::settings::{SettingsOption, SettingsRow, SettingsRowKind, row};

/// The toggle's label (Settings, General).
pub const LABEL: &str = "Launch Cua Spaces at login";
/// The Done page's checkbox.
pub const DONE_LABEL: &str = "Launch at login";
/// The one line under the toggle (and under the Done checkbox).
pub const NOTE: &str = "Keeps your Spaces, agents and Cua Volume available after a restart.";
/// [`NOTE`] while the Cua Volume experiment is off (the app does not show
/// the Volume, so the line does not promise it).
pub const NOTE_WITHOUT_VOLUME: &str = "Keeps your Spaces and agents available after a restart.";

/// The one line under the toggle and the Done checkbox: [`NOTE`], or
/// [`NOTE_WITHOUT_VOLUME`] while the Cua Volume experiment is off.
pub fn note(volume: bool) -> &'static str {
    if volume { NOTE } else { NOTE_WITHOUT_VOLUME }
}
/// macOS asks the user to allow it: where.
pub const APPROVE: &str = "Approve in System Settings \u{203a} Login Items";
/// Its button (opens Login Items).
pub const APPROVE_BUTTON: &str = "Open Login Items";
/// The system cannot find this copy of the app as a login item.
pub const NOT_FOUND: &str = "Login items are not available for this copy of Cua Spaces.";

/// What the operating system reports for the app as a login item
/// (`SMAppService.Status` on macOS).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LoginItemStatus {
    /// Registered and allowed: the app opens at login.
    Enabled,
    /// Not registered.
    #[default]
    NotRegistered,
    /// Registered, waiting for the user to allow it in System Settings,
    /// Login Items.
    RequiresApproval,
    /// The system cannot find the app as a login item.
    NotFound,
}

impl LoginItemStatus {
    /// The app is registered (it opens at login, or will once approved).
    pub fn is_on(self) -> bool {
        matches!(self, Self::Enabled | Self::RequiresApproval)
    }
}

/// The Settings toggle's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoginItemInput {
    /// What the system reports.
    pub status: LoginItemStatus,
    /// A change is running.
    pub busy: bool,
    /// Why the last change failed.
    pub error: Option<String>,
    /// This machine provides Spaces to your other devices (host setup's
    /// "A spare machine for Spaces").
    pub provides_spaces: bool,
    /// This machine runs persistent agents.
    pub runs_agents: bool,
}

/// What turning it off stops, in one line; none when nothing runs here.
pub fn off_warning(provides_spaces: bool, runs_agents: bool) -> Option<String> {
    let what = match (provides_spaces, runs_agents) {
        (true, true) => "This machine provides Spaces and runs persistent agents",
        (true, false) => "This machine provides Spaces",
        (false, true) => "This machine runs persistent agents",
        (false, false) => return None,
    };
    Some(format!(
        "{what}, which stop after a restart until you open Cua Spaces."
    ))
}

/// The General section's rows: the toggle and one line under it.
pub fn rows(input: &LoginItemInput) -> Vec<SettingsRow> {
    rows_with(input, true)
}

/// [`rows`], with the line saying whether Cua Volume is kept (`volume`:
/// the Cua Volume experiment is on).
pub fn rows_with(input: &LoginItemInput, volume: bool) -> Vec<SettingsRow> {
    use SettingsRowKind::*;
    let on = input.status.is_on();
    let mut toggle = row("launch-at-login", Toggle, LABEL);
    toggle.options = vec![
        SettingsOption {
            id: "on".into(),
            label: "On".into(),
            active: on,
        },
        SettingsOption {
            id: "off".into(),
            label: "Off".into(),
            active: !on,
        },
    ];
    toggle.enabled = !input.busy && input.status != LoginItemStatus::NotFound;
    let mut out = vec![toggle];
    if let Some(error) = input.error.clone().filter(|e| !e.is_empty()) {
        out.push(row("launch-at-login-error", Error, &error));
        return out;
    }
    match input.status {
        LoginItemStatus::RequiresApproval => {
            let mut r = row("launch-at-login-approve", Text, APPROVE);
            r.button = Some(APPROVE_BUTTON.into());
            out.push(r);
        }
        LoginItemStatus::NotFound => out.push(row("launch-at-login-note", Note, NOT_FOUND)),
        LoginItemStatus::NotRegistered => {
            let line = off_warning(input.provides_spaces, input.runs_agents);
            out.push(row(
                "launch-at-login-note",
                Note,
                line.as_deref().unwrap_or(note(volume)),
            ));
        }
        LoginItemStatus::Enabled => out.push(row("launch-at-login-note", Note, note(volume))),
    }
    out
}

/// What the app does about it at launch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginItemPlan {
    /// Register the app as a login item now.
    pub register: bool,
    /// Save this as the user's choice (`AppSettings::launch_at_login`).
    pub record: Option<bool>,
}

/// At launch: an install whose first run is finished and where no choice
/// was ever made (the first run predates the setting) is turned on when
/// this machine provides Spaces or runs persistent agents, and an install
/// already registered records that. Nothing else changes: the user's
/// choice, once made, stands, and the first run decides on its Done page.
pub fn launch_plan(
    choice: Option<bool>,
    onboarded: bool,
    serves: bool,
    status: LoginItemStatus,
) -> LoginItemPlan {
    if choice.is_some() || !onboarded {
        return LoginItemPlan::default();
    }
    if status.is_on() {
        return LoginItemPlan {
            register: false,
            record: Some(true),
        };
    }
    if serves && status == LoginItemStatus::NotRegistered {
        return LoginItemPlan {
            register: true,
            record: Some(true),
        };
    }
    LoginItemPlan::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(rows: &[SettingsRow]) -> Vec<String> {
        rows.iter()
            .map(|r| format!("{}:{}", r.id, r.label))
            .collect()
    }

    #[test]
    fn the_toggle_shows_what_the_system_reports() {
        for (status, on, enabled) in [
            (LoginItemStatus::Enabled, true, true),
            (LoginItemStatus::NotRegistered, false, true),
            (LoginItemStatus::RequiresApproval, true, true),
            (LoginItemStatus::NotFound, false, false),
        ] {
            let r = rows(&LoginItemInput {
                status,
                ..Default::default()
            });
            assert_eq!(r[0].kind, SettingsRowKind::Toggle);
            assert_eq!(r[0].options[0].active, on, "{status:?}");
            assert_eq!(r[0].enabled, enabled, "{status:?}");
        }
        let busy = rows(&LoginItemInput {
            status: LoginItemStatus::Enabled,
            busy: true,
            ..Default::default()
        });
        assert!(!busy[0].enabled);
    }

    #[test]
    fn the_line_under_it() {
        let approve = rows(&LoginItemInput {
            status: LoginItemStatus::RequiresApproval,
            ..Default::default()
        });
        assert_eq!(approve[1].label, APPROVE);
        assert_eq!(approve[1].button.as_deref(), Some(APPROVE_BUTTON));
        let off = rows(&LoginItemInput {
            status: LoginItemStatus::NotRegistered,
            provides_spaces: true,
            ..Default::default()
        });
        assert_eq!(
            ids(&off)[1],
            "launch-at-login-note:This machine provides Spaces, which stop after a restart until you open Cua Spaces."
        );
        let on = rows(&LoginItemInput {
            status: LoginItemStatus::Enabled,
            provides_spaces: true,
            runs_agents: true,
            ..Default::default()
        });
        assert_eq!(on[1].label, NOTE);
        let failed = rows(&LoginItemInput {
            error: Some("denied".into()),
            ..Default::default()
        });
        assert_eq!(ids(&failed)[1], "launch-at-login-error:denied");
    }

    #[test]
    fn the_app_turns_it_on_only_for_an_unchosen_serving_machine() {
        use LoginItemStatus::*;
        let on = LoginItemPlan {
            register: true,
            record: Some(true),
        };
        assert_eq!(launch_plan(None, true, true, NotRegistered), on);
        assert_eq!(
            launch_plan(None, true, false, NotRegistered),
            LoginItemPlan::default()
        );
        assert_eq!(
            launch_plan(None, true, false, Enabled),
            LoginItemPlan {
                register: false,
                record: Some(true)
            }
        );
        // A choice stands; the first run decides on Done.
        assert_eq!(
            launch_plan(Some(false), true, true, NotRegistered),
            LoginItemPlan::default()
        );
        assert_eq!(
            launch_plan(None, false, true, NotRegistered),
            LoginItemPlan::default()
        );
        assert_eq!(
            launch_plan(None, true, true, NotFound),
            LoginItemPlan::default()
        );
    }
}
