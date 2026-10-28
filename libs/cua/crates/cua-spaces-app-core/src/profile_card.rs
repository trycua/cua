// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The card a person or an agent opens to in a Space (their name in the
//! thread, a mention, a row in "In this space"): one calm shape for both.
//!
//! - **A person** ([`person`]): name, email, their access here (Owner, Can
//!   edit, Can view), team, Active now or last seen; Change access and
//!   Remove from Space for the owner and admins.
//! - **An agent** ([`agent`]): name with an Agent badge, one line on who runs
//!   it, where and on what ("Run by Ada in this Space, uses Anthropic"), its
//!   status (Working on "…", Idle, Not connected with Connect), Configure,
//!   Remove from Space; everything else (handle, harness and
//!   version, how and when it joined, who may task, stop or approve it) folded
//!   under Details, with ids copyable. The owner is a person (their name,
//!   never their id); the three rights show as one "Default permissions" line
//!   while they are all at their defaults.

use serde::{Deserialize, Serialize};

use crate::connectors::{self, ConnectorsInput};

/// A row under Details.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CardDetail {
    pub label: String,
    pub value: String,
    /// An id: a Copy button beside it.
    pub copy: bool,
}

fn detail(label: &str, value: impl Into<String>, copy: bool) -> CardDetail {
    CardDetail {
        label: label.into(),
        value: value.into(),
        copy,
    }
}

/// "Just now", "5 minutes ago", "3 hours ago", "2 days ago".
pub fn ago(then_ms: u64, now_ms: u64) -> String {
    let s = now_ms.saturating_sub(then_ms) / 1000;
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match s {
        0..=59 => "just now".into(),
        60..=3599 => plural(s / 60, "minute"),
        3600..=86_399 => plural(s / 3600, "hour"),
        _ => plural(s / 86_400, "day"),
    }
}

// ------------------------------------------------------------------ people

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PersonCardInput {
    pub name: String,
    /// Who the Space is shared with as (an email or account id), when known.
    pub who: Option<String>,
    /// It's you.
    pub me: bool,
    /// `owner`, `editor`, `viewer` (absent: not known).
    pub access: Option<String>,
    pub team: Option<String>,
    /// Connected to the Space now.
    pub active: bool,
    pub last_seen_ms: Option<u64>,
    pub now_ms: u64,
    /// You are the Space's owner or an admin.
    pub can_manage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessChoice {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonCardView {
    pub name: String,
    /// "(you)" after the name.
    pub you_label: Option<String>,
    pub email: Option<String>,
    /// "Owner", "Can edit", "Can view".
    pub access: Option<String>,
    pub team: Option<String>,
    /// "Active now", "Last seen 3 hours ago".
    pub presence: Option<String>,
    pub active: bool,
    /// Change access: the choices and the current one (owner and admins, not on the owner or yourself).
    pub change_access_label: Option<String>,
    pub access_choices: Vec<AccessChoice>,
    pub access_value: Option<String>,
    pub remove_label: Option<String>,
}

fn access_label(a: &str) -> Option<&'static str> {
    match a {
        "owner" => Some("Owner"),
        "editor" | "edit" => Some("Can edit"),
        "viewer" | "view" => Some("Can view"),
        _ => None,
    }
}

pub fn person(i: &PersonCardInput) -> PersonCardView {
    let email = i.who.clone().filter(|w| w.contains('@'));
    let owner = i.access.as_deref() == Some("owner");
    let manage = i.can_manage && !i.me && !owner && i.who.is_some();
    PersonCardView {
        name: i.name.clone(),
        you_label: i.me.then(|| "(you)".into()),
        access: i.access.as_deref().and_then(access_label).map(Into::into),
        team: i.team.clone(),
        presence: if i.active {
            Some("Active now".into())
        } else {
            i.last_seen_ms
                .map(|t| format!("Last seen {}", ago(t, i.now_ms)))
        },
        active: i.active,
        change_access_label: manage.then(|| "Change access".into()),
        access_choices: if manage {
            vec![
                AccessChoice {
                    value: "viewer".into(),
                    label: "Can view".into(),
                },
                AccessChoice {
                    value: "editor".into(),
                    label: "Can edit".into(),
                },
            ]
        } else {
            vec![]
        },
        access_value: manage
            .then_some(match i.access.as_deref() {
                Some("editor" | "edit") => "editor",
                _ => "viewer",
            })
            .map(Into::into),
        remove_label: manage.then(|| "Remove from Space".into()),
        email,
    }
}

// ------------------------------------------------------------------ agents

/// The Connectors row of the harness an agent runs as: every agent harness
/// ([`connectors::HARNESS_APPS`]), and the apps a connected agent may report.
fn app_of(harness: &str) -> Option<&'static str> {
    connectors::app_for_harness(harness).or(match harness {
        "codex" => Some("codex"),
        "cursor" => Some("cursor"),
        "copilot-cli" => Some("copilot-cli"),
        _ => None,
    })
}

/// One right, as the page words it ("Can give it tasks", "Ada").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CardRight {
    pub label: String,
    pub holders: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentCardInput {
    pub display_name: String,
    pub handle: String,
    /// `claude-code`, `hermes`, ...
    pub harness: String,
    /// "Claude Code 2.1.3", as it reported it (absent: it didn't say).
    pub harness_reported: Option<String>,
    pub owner_name: String,
    pub owner_id: String,
    pub owner_is_me: bool,
    /// The computer it posts from (an agent connected from outside).
    pub device: Option<String>,
    /// `space` (started in this Space) or `external`.
    pub origin: String,
    /// The member's status (`working`, `idle`, `asleep`, `pending`, ...).
    pub status: String,
    /// What it is doing, when it says.
    pub activity: Option<String>,
    /// "Started in this Space", "Joined with a code".
    pub joined_how: String,
    pub joined_at_ms: Option<u64>,
    pub now_ms: u64,
    pub rights: Vec<CardRight>,
    /// Every right is at its default (who can task, stop and approve).
    pub rights_default: bool,
    /// You may change its rights (the Space's owner and admins, and its owner).
    pub can_edit_rights: bool,
    /// You may remove it.
    pub can_remove: bool,
    /// This device's sign-ins (for an agent of yours: Not connected).
    pub connectors: ConnectorsInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCardView {
    pub name: String,
    pub badge: String,
    /// "Run by Ada in this Space, uses Anthropic".
    pub line: String,
    /// `working`, `idle`, `not-connected`, `pending`.
    pub status: String,
    pub status_text: String,
    /// "Connect Claude Code", while Not connected.
    pub connect_label: Option<String>,
    /// The Connectors app it runs as (Connect and Open its settings).
    pub app: Option<String>,
    pub settings_label: Option<String>,
    pub remove_label: Option<String>,
    pub details_label: String,
    pub details: Vec<CardDetail>,
    /// "Owner": the label of the row that shows who runs it.
    pub owner_label: String,
    /// Who runs it, as a person: their account id (to find their card) and name.
    pub owner_id: String,
    pub owner_name: String,
    /// "Default permissions", while every right is at its default.
    pub default_permissions: Option<String>,
    /// "Edit", beside it, for those who may change the rights.
    pub edit_rights_label: Option<String>,
}

pub fn agent(i: &AgentCardInput) -> AgentCardView {
    let app = app_of(&i.harness);
    let name = if i.display_name.is_empty() {
        i.handle.clone()
    } else {
        i.display_name.clone()
    };
    let who = if i.owner_is_me {
        "you".to_string()
    } else {
        i.owner_name.clone()
    };
    let place = match (&i.device, i.origin.as_str()) {
        (Some(d), _) => format!("on {d}"),
        (None, "space") => "in this Space".to_string(),
        _ => String::new(),
    };
    // The provider it signs in to: the one connected here, else the first it uses.
    let providers = app.map(connectors::app_providers).unwrap_or_default();
    let provider = providers
        .iter()
        .find(|p| {
            app.is_some_and(|a| i.owner_is_me && connectors::signed_in(&i.connectors, a))
                && connectors::provider_label(p).is_some()
        })
        .or(providers.first())
        .and_then(|p| connectors::provider_label(p));
    let mut line = format!("Run by {who}");
    if !place.is_empty() {
        line.push(' ');
        line.push_str(&place);
    }
    if let Some(p) = provider {
        line.push_str(&format!(", uses {p}"));
    }
    // Your own agent with no sign-in it can use: Not connected, with Connect.
    let not_connected =
        i.owner_is_me && app.is_some_and(|a| !connectors::signed_in(&i.connectors, a));
    let label = app_label(&i.harness).unwrap_or(&name).to_string();
    let (status, status_text) = if i.status == "pending" {
        ("pending", "Waiting to be let in".to_string())
    } else if not_connected {
        ("not-connected", "Not connected".to_string())
    } else if i.status == "working" || i.status == "busy" {
        (
            "working",
            match &i.activity {
                // The stage in a word ("Thinking", "Using Bash"), never the task's text.
                Some(a) if !a.is_empty() => a.clone(),
                _ => "Working".into(),
            },
        )
    } else {
        ("idle", "Idle".to_string())
    };
    let in_settings = i.owner_is_me
        && app.is_some_and(|a| {
            i.connectors
                .apps
                .iter()
                .any(|x| x.agent == a && x.installed)
        });
    let mut details = vec![detail("Handle", format!("@{}", i.handle), true)];
    details.push(detail(
        "Runs",
        i.harness_reported
            .clone()
            .unwrap_or_else(|| "Not reported".into()),
        false,
    ));
    let joined = match i.joined_at_ms {
        Some(t) => format!("{}, {}", i.joined_how, ago(t, i.now_ms)),
        None => i.joined_how.clone(),
    };
    details.push(detail("Joined", joined, false));
    // Each right is a row, until all three are at their defaults: then one line.
    if !i.rights_default {
        details.extend(
            i.rights
                .iter()
                .map(|r| detail(&r.label, r.holders.clone(), false)),
        );
    }
    let has_rights = !i.rights.is_empty();
    AgentCardView {
        name,
        badge: "Agent".into(),
        line,
        status: status.into(),
        status_text,
        connect_label: not_connected.then(|| format!("Connect {label}")),
        app: app.map(Into::into),
        settings_label: in_settings.then(|| "Configure".into()),
        remove_label: i.can_remove.then(|| "Remove from Space".into()),
        details_label: "Details".into(),
        details,
        owner_label: "Owner".into(),
        owner_id: i.owner_id.clone(),
        owner_name: i.owner_name.clone(),
        default_permissions: (has_rights && i.rights_default).then(|| "Default permissions".into()),
        edit_rights_label: (has_rights && i.rights_default && i.can_edit_rights)
            .then(|| "Edit".into()),
    }
}

fn app_label(harness: &str) -> Option<&'static str> {
    match harness {
        "cursor" => Some("Cursor"),
        "copilot-cli" => Some("GitHub Copilot CLI"),
        "codex" => Some("Codex"),
        "openai-codex" => Some("Codex"),
        h => connectors::HARNESS_APPS
            .iter()
            .find(|x| x.harness == h)
            .map(|x| x.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_keys::{AgentKeyInput, AgentKeysInput};

    const NOW: u64 = 1_000_000_000;

    #[test]
    fn owner_sees_an_editor_with_change_access_and_remove() {
        let v = person(&PersonCardInput {
            name: "Grace".into(),
            who: Some("grace@example.com".into()),
            access: Some("editor".into()),
            team: Some("Acme".into()),
            active: true,
            now_ms: NOW,
            can_manage: true,
            ..Default::default()
        });
        assert_eq!(v.email.as_deref(), Some("grace@example.com"));
        assert_eq!(v.access.as_deref(), Some("Can edit"));
        assert_eq!(v.presence.as_deref(), Some("Active now"));
        assert_eq!(v.change_access_label.as_deref(), Some("Change access"));
        assert_eq!(v.access_value.as_deref(), Some("editor"));
        assert_eq!(v.remove_label.as_deref(), Some("Remove from Space"));
    }

    #[test]
    fn your_own_card_and_a_viewer_who_cant_manage_have_no_access_actions() {
        let me = person(&PersonCardInput {
            name: "Ada".into(),
            who: Some("ada@example.com".into()),
            me: true,
            access: Some("owner".into()),
            last_seen_ms: Some(NOW - 3 * 3600 * 1000),
            now_ms: NOW,
            can_manage: true,
            ..Default::default()
        });
        assert_eq!(me.you_label.as_deref(), Some("(you)"));
        assert_eq!(me.access.as_deref(), Some("Owner"));
        assert_eq!(me.presence.as_deref(), Some("Last seen 3 hours ago"));
        assert!(me.change_access_label.is_none() && me.remove_label.is_none());
        let other = person(&PersonCardInput {
            name: "Linus".into(),
            who: Some("linus@example.com".into()),
            access: Some("viewer".into()),
            can_manage: false,
            ..Default::default()
        });
        assert!(other.change_access_label.is_none() && other.remove_label.is_none());
        assert_eq!(other.email.as_deref(), Some("linus@example.com"));
    }

    fn agent_input() -> AgentCardInput {
        AgentCardInput {
            display_name: "Claude Code".into(),
            handle: "claude".into(),
            harness: "claude-code".into(),
            harness_reported: Some("Claude Code 2.1.3".into()),
            owner_name: "Ada".into(),
            owner_id: "acct_ada".into(),
            owner_is_me: true,
            origin: "space".into(),
            status: "working".into(),
            activity: Some("fix the build".into()),
            joined_how: "Started in this Space".into(),
            joined_at_ms: Some(NOW - 120_000),
            now_ms: NOW,
            rights: vec![CardRight {
                label: "Can give it tasks".into(),
                holders: "Owner".into(),
            }],
            can_remove: true,
            connectors: ConnectorsInput {
                apps: vec![crate::connectors::ConnectorAppInput {
                    agent: "claude-code".into(),
                    name: "Claude Code".into(),
                    installed: true,
                    ..Default::default()
                }],
                keys: AgentKeysInput {
                    keys: vec![AgentKeyInput {
                        provider: "other".into(),
                        env: "CLAUDE_CODE_OAUTH_TOKEN".into(),
                        last4: String::new(),
                        added_ms: 1,
                        expired: false,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn an_agent_card_says_who_runs_it_where_and_on_what_in_one_line() {
        let v = agent(&agent_input());
        assert_eq!(v.badge, "Agent");
        assert_eq!(v.line, "Run by you in this Space, uses Anthropic");
        // The stage, never the task.
        assert_eq!(v.status_text, "fix the build");
        assert_eq!(v.settings_label.as_deref(), Some("Configure"));
        assert_eq!(v.remove_label.as_deref(), Some("Remove from Space"));
        assert!(v.connect_label.is_none());
        let labels: Vec<_> = v.details.iter().map(|d| d.label.as_str()).collect();
        assert_eq!(labels, ["Handle", "Runs", "Joined", "Can give it tasks"]);
        assert!(v.details[0].copy);
        assert_eq!(v.details[2].value, "Started in this Space, 2 minutes ago");
    }

    #[test]
    fn the_owner_is_a_person_never_an_id() {
        let v = agent(&agent_input());
        assert_eq!(v.owner_label, "Owner");
        assert_eq!(
            (v.owner_name.as_str(), v.owner_id.as_str()),
            ("Ada", "acct_ada")
        );
        // No row of the card shows the account id.
        assert!(v.details.iter().all(|d| !d.value.contains("acct_")));
    }

    #[test]
    fn default_rights_are_one_line_with_an_edit_link_for_those_who_may() {
        let mut i = agent_input();
        i.rights_default = true;
        i.can_edit_rights = true;
        let v = agent(&i);
        assert_eq!(
            v.default_permissions.as_deref(),
            Some("Default permissions")
        );
        assert_eq!(v.edit_rights_label.as_deref(), Some("Edit"));
        assert!(v.details.iter().all(|d| d.label != "Can give it tasks"));
        // Someone who can't change them reads the line only.
        i.can_edit_rights = false;
        let v = agent(&i);
        assert!(v.default_permissions.is_some() && v.edit_rights_label.is_none());
        // Changed rights are the three rows again.
        i.rights_default = false;
        let v = agent(&i);
        assert!(v.default_permissions.is_none() && v.edit_rights_label.is_none());
        assert!(v.details.iter().any(|d| d.label == "Can give it tasks"));
    }

    #[test]
    fn someone_elses_agent_on_their_computer_has_no_settings_and_isnt_checked_here() {
        let mut i = agent_input();
        i.owner_is_me = false;
        i.owner_name = "Linus".into();
        i.device = Some("linus-thinkpad".into());
        i.origin = "external".into();
        i.status = "idle".into();
        i.connectors.keys.keys.clear();
        i.can_remove = false;
        let v = agent(&i);
        assert_eq!(v.line, "Run by Linus on linus-thinkpad, uses Anthropic");
        assert_eq!(v.status_text, "Idle");
        assert!(
            v.settings_label.is_none() && v.remove_label.is_none() && v.connect_label.is_none()
        );
    }

    #[test]
    fn your_agent_with_no_sign_in_is_not_connected_with_connect() {
        let mut i = agent_input();
        i.connectors.keys.keys.clear();
        let v = agent(&i);
        assert_eq!(v.status, "not-connected");
        assert_eq!(v.status_text, "Not connected");
        assert_eq!(v.connect_label.as_deref(), Some("Connect Claude Code"));
    }

    #[test]
    fn ago_reads_plainly() {
        assert_eq!(ago(NOW - 5_000, NOW), "just now");
        assert_eq!(ago(NOW - 60_000, NOW), "1 minute ago");
        assert_eq!(ago(NOW - 2 * 86_400_000, NOW), "2 days ago");
    }
}
