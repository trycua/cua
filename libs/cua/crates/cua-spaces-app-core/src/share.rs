// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Share": who a Space is shared with, and the sheet that shares it.
//!
//! Plain data in (the Space and its shares, as the `space_shares` tool
//! returns them) and plain data out (one line per person, the add field,
//! the role menu, and the command to run). Sharing hands the Space to
//! another cua.ai account, so the shell runs the command through the
//! daemon, which asks for presence (Touch ID or the login password) before
//! anything reaches the relay. Removing someone needs no presence and
//! takes effect at once.

use serde::{Deserialize, Serialize};

/// One person the Space is shared with, as the tool reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareEntryInput {
    /// Email or account id.
    pub who: String,
    /// `viewer` or `editor`.
    pub role: String,
    /// Connected through the relay now.
    #[serde(default)]
    pub connected: bool,
}

/// Everything the Share sheet reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareInput {
    /// The Space's id.
    pub space_id: String,
    /// Its display name.
    pub space_name: String,
    /// Who it is shared with.
    #[serde(default)]
    pub shares: Vec<ShareEntryInput>,
    /// Signed in to cua.ai (sharing goes through the account's relay).
    #[serde(default)]
    pub signed_in: bool,
    /// The Space can be shared (a host, or a driver with `relay_attach`).
    #[serde(default = "yes")]
    pub shareable: bool,
}

fn yes() -> bool {
    true
}

/// The command the shell runs (through the daemon's sharing tools).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ShareRequest {
    /// `share_space` (also changes a role).
    Share {
        /// Space id.
        space: String,
        /// Email or account id.
        who: String,
        /// `viewer` or `editor`.
        role: String,
    },
    /// `unshare_space`.
    Unshare {
        /// Space id.
        space: String,
        /// Email or account id.
        who: String,
    },
}

/// The sheet's state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSheetState {
    /// The add field, as typed.
    pub who: String,
    /// The role the add field shares with.
    pub role: String,
    /// A command runs.
    pub busy: bool,
    /// The last failure.
    pub error: Option<String>,
    /// The command waiting for the shell.
    pub request: Option<ShareRequest>,
}

/// An input to the sheet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ShareSheetAction {
    /// The add field changed.
    SetWho {
        /// Text.
        who: String,
    },
    /// The add field's role menu changed.
    SetRole {
        /// `viewer` or `editor`.
        role: String,
    },
    /// Share was pressed.
    Submit,
    /// A person's role menu changed.
    ChangeRole {
        /// Email or account id.
        who: String,
        /// `viewer` or `editor`.
        role: String,
    },
    /// A person's Remove was pressed.
    Remove {
        /// Email or account id.
        who: String,
    },
    /// The command finished.
    Done,
    /// The command failed (or presence was declined).
    Failed {
        /// Why.
        error: String,
    },
}

/// A role in the menus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleOption {
    /// `viewer` or `editor`.
    pub id: String,
    /// `Can view` or `Can edit`.
    pub label: String,
}

/// One person, one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRowView {
    /// Email or account id.
    pub who: String,
    /// `viewer` or `editor`.
    pub role: String,
    /// Connected through the relay now (the shells show a status dot).
    pub connected: bool,
}

/// A button with somewhere to go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareActionView {
    /// The button's words.
    pub label: String,
    /// Where it opens.
    pub url: String,
}

/// The sheet as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSheetView {
    /// `Share “studio”`.
    pub title: String,
    /// One person per line.
    pub rows: Vec<ShareRowView>,
    /// Shown with no rows.
    pub empty_text: String,
    /// The add field's placeholder.
    pub who_placeholder: String,
    /// The add field, as typed.
    pub who: String,
    /// The add field's role.
    pub role: String,
    /// The role menu (for the add field and every row).
    pub roles: Vec<RoleOption>,
    /// Share is enabled.
    pub can_share: bool,
    /// Share.
    pub share_label: String,
    /// Remove (per row).
    pub remove_label: String,
    /// Done.
    pub done_label: String,
    /// Why the sheet cannot share at all (not signed in, an old driver).
    pub disabled_reason: Option<String>,
    /// What the add field lacks (shown under it, only once typed).
    pub hint: Option<String>,
    /// A command runs.
    pub busy: bool,
    /// The last failure.
    pub error: Option<String>,
    /// The command waiting for the shell.
    pub request: Option<ShareRequest>,
}

/// Why `who` is not an email or account id, or its normalized form.
/// A team (`team:<id>`) only while Teams are on ([`cua_home::experimental`]).
pub fn validate_who(who: &str) -> Result<String, String> {
    validate_who_with(who, cua_home::experimental::teams_enabled())
}

fn validate_who_with(who: &str, teams: bool) -> Result<String, String> {
    let w = who.trim();
    if w.is_empty() {
        return Err("Enter an email or account ID.".into());
    }
    if !teams && ["team:", "org:", "group:"].iter().any(|p| w.starts_with(p)) {
        return Err(format!(
            "{} Share with people, one at a time.",
            cua_home::experimental::TEAMS_UNAVAILABLE
        ));
    }
    if let Some(team) = w.strip_prefix("team:") {
        // A team (`team:<id>` as `cua team ls` or the Teams page shows it):
        // an organization's id or alias. Every member gets the Space, from
        // a device an admin approved. The relay's old `team-<16 hex>` ids
        // are refused.
        let legacy = team
            .strip_prefix("team-")
            .is_some_and(|h| h.len() == 16 && h.chars().all(|c| c.is_ascii_hexdigit()));
        let ok = !team.is_empty()
            && team.len() <= 64
            && team
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        return if legacy {
            Err("That team ID is from the old Teams. Use the team ID `cua team ls` shows.".into())
        } else if ok {
            Ok(format!("team:{team}"))
        } else {
            Err("Enter a team as team:<id>, as `cua team ls` shows it.".into())
        };
    }
    if ["org:", "group:"].iter().any(|p| w.starts_with(p)) {
        return Err("Share with people, or with a team as team:<id>.".into());
    }
    if w.len() > 254 || w.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("Enter an email or account ID.".into());
    }
    if let Some((local, domain)) = w.split_once('@') {
        if local.is_empty() || !domain.contains('.') || domain.ends_with('.') {
            return Err("Enter a full email address.".into());
        }
        return Ok(w.to_ascii_lowercase());
    }
    if w.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
    {
        Ok(w.to_string())
    } else {
        Err("Enter an email or account ID.".into())
    }
}

fn role_of(role: &str) -> &'static str {
    if role == "editor" { "editor" } else { "viewer" }
}

/// A role, for a person or a whole team: "Can view" or "Can edit".
fn role_for(_who: &str, role: &str) -> &'static str {
    role_of(role)
}

/// Why a Free or Pro account without a team cannot share (CUA-1883).
pub const NO_TEAM: &str = "Sharing is for teams. Start a team to share this Space.";

/// Where Teams starts, at one seat.
pub const TEAMS_URL: &str = "https://app.cua.ai/buy/teams?from=app_share";

/// Where an owner invites people to their team.
pub const INVITE_URL: &str = "https://app.cua.ai/team";

/// Why the sheet cannot share, and what to do about it: sharing is for
/// teams (CUA-1883).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareGate {
    /// "Sharing is for teams. Start a team to share this Space."
    pub reason: String,
    /// "See Teams": where Teams starts, at one seat.
    pub action: ShareActionView,
}

/// The gate for an account, from whether it is in a team that can share:
/// `Some(false)` closes the sheet; `Some(true)`, or `None` (the host could
/// not say, so the server decides), leaves it open.
pub fn share_gate(has_team: Option<bool>) -> Option<ShareGate> {
    (has_team == Some(false)).then(|| ShareGate {
        reason: NO_TEAM.into(),
        action: ShareActionView {
            label: "See Teams".into(),
            url: TEAMS_URL.into(),
        },
    })
}

/// What a refused share says, in plain words, from the server's code or
/// message (`share_outside_team`, `sharing_needs_team`). `None` for any
/// other error: the caller shows its own.
pub fn share_error_words(error: &str, who: &str) -> Option<String> {
    let e = error.to_ascii_lowercase();
    if e.contains("sharing_needs_team") {
        Some(NO_TEAM.into())
    } else if e.contains("share_outside_team") {
        let who = who.trim();
        let name = if who.is_empty() { "That person" } else { who };
        Some(format!(
            "{name} isn't in your team. Invite them to your team first."
        ))
    } else {
        None
    }
}

/// Someone who lost access to a computer when sharing became team-only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRemovalInput {
    /// The person (an email or account id).
    pub who: String,
    /// The computer's name.
    #[serde(default)]
    pub machine_name: String,
}

/// The one-line note for an owner whose shares were removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRemovalNote {
    /// One line: who can no longer open what, and why.
    pub text: String,
    /// "Invite to your team".
    pub action: ShareActionView,
    /// Dismiss.
    pub dismiss: String,
}

/// The note for `removals`, or nothing when there are none. It names up to
/// three people, then counts the rest, and names the computer only when
/// there is one.
pub fn share_removal_note(removals: &[ShareRemovalInput]) -> Option<ShareRemovalNote> {
    let mut people: Vec<&str> = Vec::new();
    for r in removals {
        let w = r.who.trim();
        if !w.is_empty() && !people.iter().any(|p| p.eq_ignore_ascii_case(w)) {
            people.push(w);
        }
    }
    if people.is_empty() {
        return None;
    }
    let mut computers: Vec<&str> = removals
        .iter()
        .map(|r| r.machine_name.trim())
        .filter(|n| !n.is_empty())
        .collect();
    computers.sort_unstable();
    computers.dedup();
    let shown: Vec<String> = people.iter().take(3).map(|p| p.to_string()).collect();
    let rest = people.len() - shown.len();
    let who = match (shown.as_slice(), rest) {
        ([a], 0) => a.clone(),
        ([a, b], 0) => format!("{a} and {b}"),
        ([a, b, c], 0) => format!("{a}, {b} and {c}"),
        (_, n) => format!("{}, and {n} more", shown.join(", ")),
    };
    let what = match computers.as_slice() {
        [one] => format!("\u{201c}{one}\u{201d}"),
        _ => "your shared Spaces".to_string(),
    };
    Some(ShareRemovalNote {
        text: format!("{who} can no longer open {what}, because they're not in your team."),
        action: ShareActionView {
            label: "Invite to your team".into(),
            url: INVITE_URL.into(),
        },
        dismiss: "Dismiss".into(),
    })
}

/// The roles, in menu order.
pub fn roles() -> Vec<RoleOption> {
    vec![
        RoleOption {
            id: "viewer".into(),
            label: "Can view".into(),
        },
        RoleOption {
            id: "editor".into(),
            label: "Can edit".into(),
        },
    ]
}

/// A new sheet.
pub fn share_initial() -> ShareSheetState {
    ShareSheetState {
        role: "viewer".into(),
        ..Default::default()
    }
}

fn can_act(input: &ShareInput) -> bool {
    input.signed_in && input.shareable
}

/// Advances the sheet.
pub fn share_reduce(
    input: &ShareInput,
    state: &ShareSheetState,
    action: &ShareSheetAction,
) -> ShareSheetState {
    let mut s = state.clone();
    let space = input.space_id.clone();
    match action {
        ShareSheetAction::SetWho { who } if !s.busy => {
            s.who = who.clone();
            s.error = None;
        }
        ShareSheetAction::SetRole { role } if !s.busy => s.role = role_of(role).into(),
        ShareSheetAction::Submit if !s.busy && can_act(input) => {
            if let Ok(who) = validate_who(&s.who) {
                s.busy = true;
                s.error = None;
                let role = role_for(&who, &s.role).into();
                s.request = Some(ShareRequest::Share { space, who, role });
            }
        }
        ShareSheetAction::ChangeRole { who, role } if !s.busy && can_act(input) => {
            s.busy = true;
            s.error = None;
            s.request = Some(ShareRequest::Share {
                space,
                who: who.clone(),
                role: role_of(role).into(),
            });
        }
        ShareSheetAction::Remove { who } if !s.busy && input.signed_in => {
            s.busy = true;
            s.error = None;
            s.request = Some(ShareRequest::Unshare {
                space,
                who: who.clone(),
            });
        }
        ShareSheetAction::Done => {
            if matches!(s.request, Some(ShareRequest::Share { .. })) {
                s.who.clear();
            }
            s.busy = false;
            s.request = None;
            s.error = None;
        }
        ShareSheetAction::Failed { error } => {
            s.busy = false;
            s.request = None;
            s.error = Some(error.clone());
        }
        _ => {}
    }
    s
}

/// The sheet as drawn.
pub fn share_view(input: &ShareInput, state: &ShareSheetState) -> ShareSheetView {
    let valid = validate_who(&state.who);
    let disabled_reason = if !input.signed_in {
        Some("Sign in to cua.ai to share.".into())
    } else if !input.shareable {
        Some("This Space's driver is too old to share. Update its image.".into())
    } else {
        None
    };
    ShareSheetView {
        title: format!("Share \u{201c}{}\u{201d}", input.space_name),
        rows: input
            .shares
            .iter()
            .map(|e| ShareRowView {
                who: e.who.clone(),
                role: role_for(&e.who, &e.role).into(),
                connected: e.connected,
            })
            .collect(),
        empty_text: "Only you".into(),
        who_placeholder: "Email or account ID".into(),
        who: state.who.clone(),
        role: role_of(&state.role).into(),
        roles: roles(),
        can_share: valid.is_ok() && !state.busy && disabled_reason.is_none(),
        share_label: "Share".into(),
        remove_label: "Remove".into(),
        done_label: "Done".into(),
        disabled_reason,
        hint: if state.who.trim().is_empty() {
            None
        } else {
            valid.err()
        },
        busy: state.busy,
        error: state.error.clone(),
        request: state.request.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ShareInput {
        ShareInput {
            space_id: "local:studio".into(),
            space_name: "studio".into(),
            shares: vec![],
            signed_in: true,
            shareable: true,
        }
    }

    #[test]
    fn share_change_remove_and_fail() {
        let i = input();
        let s = share_initial();
        let v = share_view(&i, &s);
        assert_eq!(v.title, "Share \u{201c}studio\u{201d}");
        assert!(!v.can_share && v.hint.is_none() && v.rows.is_empty());
        let s = share_reduce(&i, &s, &ShareSheetAction::SetWho { who: "bob@".into() });
        assert_eq!(
            share_view(&i, &s).hint.as_deref(),
            Some("Enter a full email address.")
        );
        let s = share_reduce(&i, &s, &ShareSheetAction::Submit);
        assert!(s.request.is_none(), "invalid: nothing to run");
        let s = share_reduce(
            &i,
            &s,
            &ShareSheetAction::SetWho {
                who: " Bob@Example.com ".into(),
            },
        );
        let s = share_reduce(
            &i,
            &s,
            &ShareSheetAction::SetRole {
                role: "editor".into(),
            },
        );
        let s = share_reduce(&i, &s, &ShareSheetAction::Submit);
        assert_eq!(
            s.request,
            Some(ShareRequest::Share {
                space: "local:studio".into(),
                who: "bob@example.com".into(),
                role: "editor".into()
            })
        );
        assert!(!share_view(&i, &s).can_share, "busy");
        let s = share_reduce(&i, &s, &ShareSheetAction::Done);
        assert!(s.who.is_empty() && s.request.is_none());
        let s = share_reduce(
            &i,
            &s,
            &ShareSheetAction::Remove {
                who: "bob@example.com".into(),
            },
        );
        assert!(matches!(s.request, Some(ShareRequest::Unshare { .. })));
        let s = share_reduce(
            &i,
            &s,
            &ShareSheetAction::Failed {
                error: "not shared: you declined".into(),
            },
        );
        assert_eq!(s.error.as_deref(), Some("not shared: you declined"));
        assert!(!s.busy);
    }

    #[test]
    fn teams_are_shared_with_unless_turned_off() {
        // On (the default): a team id, for either role.
        assert_eq!(
            validate_who_with("team:Design Team", true).unwrap_err(),
            "Enter a team as team:<id>, as `cua team ls` shows it."
        );
        // An organization, by id or alias.
        assert_eq!(
            validate_who_with(" team:0f1e2d3c-0000-4000-8000-000000000001 ", true).unwrap(),
            "team:0f1e2d3c-0000-4000-8000-000000000001"
        );
        assert_eq!(
            validate_who_with("team:design", true).unwrap(),
            "team:design"
        );
        // Not the old relay team ids.
        assert!(
            validate_who_with("team:team-0a1b2c3d4e5f6a7b", true)
                .unwrap_err()
                .contains("old Teams")
        );
        // A whole team can be shared with as "Can view" or "Can edit".
        assert_eq!(role_for("team:design", "viewer"), "viewer");
        assert_eq!(role_for("team:design", "editor"), "editor");
        // Off (CUA_EXPERIMENTAL_TEAMS=0): refused, whatever the id.
        for who in ["team:team-0a1b", "team:design", "org:acme"] {
            assert_eq!(
                validate_who_with(who, false).unwrap_err(),
                "Teams is turned off (CUA_EXPERIMENTAL_TEAMS=0). Share with people, one at a time."
            );
        }
        assert_eq!(
            validate_who_with("bob@example.com", false).unwrap(),
            "bob@example.com"
        );
    }

    #[test]
    fn signed_out_or_old_drivers_cannot_share() {
        let mut i = input();
        i.signed_in = false;
        let s = share_reduce(
            &i,
            &share_initial(),
            &ShareSheetAction::SetWho {
                who: "bob@example.com".into(),
            },
        );
        let v = share_view(&i, &s);
        assert!(!v.can_share);
        assert_eq!(
            v.disabled_reason.as_deref(),
            Some("Sign in to cua.ai to share.")
        );
        assert!(
            share_reduce(&i, &s, &ShareSheetAction::Submit)
                .request
                .is_none()
        );
        i.signed_in = true;
        i.shareable = false;
        assert!(
            share_view(&i, &s)
                .disabled_reason
                .unwrap()
                .contains("too old")
        );
    }

    #[test]
    fn no_team_means_the_sheet_explains_and_links_to_teams() {
        // Unknown, or in a team: the sheet stays open.
        assert!(share_gate(None).is_none());
        assert!(share_gate(Some(true)).is_none());
        let g = share_gate(Some(false)).expect("closed without a team");
        assert_eq!(
            g.reason,
            "Sharing is for teams. Start a team to share this Space."
        );
        assert_eq!(g.action.label, "See Teams");
        assert_eq!(g.action.url, "https://app.cua.ai/buy/teams?from=app_share");
    }

    #[test]
    fn team_shares_keep_the_role_chosen() {
        let i = input();
        let mut s = share_reduce(
            &i,
            &share_initial(),
            &ShareSheetAction::SetWho {
                who: "team:design".into(),
            },
        );
        s = share_reduce(
            &i,
            &s,
            &ShareSheetAction::SetRole {
                role: "viewer".into(),
            },
        );
        s = share_reduce(&i, &s, &ShareSheetAction::Submit);
        assert_eq!(
            s.request,
            Some(ShareRequest::Share {
                space: "local:studio".into(),
                who: "team:design".into(),
                role: "viewer".into()
            })
        );
    }

    #[test]
    fn refused_shares_read_in_plain_words() {
        assert_eq!(
            share_error_words("HTTP 403: share_outside_team", "alex@x.test").as_deref(),
            Some("alex@x.test isn't in your team. Invite them to your team first.")
        );
        assert_eq!(
            share_error_words("sharing_needs_team", "").as_deref(),
            Some(NO_TEAM)
        );
        assert_eq!(
            share_error_words("share_outside_team", " ").as_deref(),
            Some("That person isn't in your team. Invite them to your team first.")
        );
        assert!(share_error_words("HTTP 500", "a@b.c").is_none());
    }

    #[test]
    fn removal_note_is_one_line_with_an_invite_button() {
        let r = |who: &str, m: &str| ShareRemovalInput {
            who: who.into(),
            machine_name: m.into(),
        };
        assert!(share_removal_note(&[]).is_none());
        let n =
            share_removal_note(&[r("alex@x.test", "Design"), r("sam@x.test", "Design")]).unwrap();
        assert_eq!(
            n.text,
            "alex@x.test and sam@x.test can no longer open \u{201c}Design\u{201d}, because they're not in your team."
        );
        assert_eq!(n.action.label, "Invite to your team");
        assert_eq!(n.action.url, "https://app.cua.ai/team");
        // Several computers, many people: still one line.
        let n = share_removal_note(&[
            r("a@x.test", "One"),
            r("b@x.test", "Two"),
            r("c@x.test", "One"),
            r("d@x.test", "One"),
            r("A@x.test", "One"),
        ])
        .unwrap();
        assert_eq!(
            n.text,
            "a@x.test, b@x.test, c@x.test, and 1 more can no longer open your shared Spaces, because they're not in your team."
        );
        assert!(!n.text.contains('\n'));
    }
}
