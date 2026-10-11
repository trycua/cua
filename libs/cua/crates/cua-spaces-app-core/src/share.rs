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
pub fn validate_who(who: &str) -> Result<String, String> {
    let w = who.trim();
    if w.is_empty() {
        return Err("Enter an email or account ID.".into());
    }
    if ["team:", "org:", "group:"].iter().any(|p| w.starts_with(p)) {
        return Err("Share with people, one at a time.".into());
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
                s.request = Some(ShareRequest::Share {
                    space,
                    who,
                    role: role_of(&s.role).into(),
                });
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
                role: role_of(&e.role).into(),
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
        assert_eq!(
            validate_who("team:design").unwrap_err(),
            "Share with people, one at a time."
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
}
