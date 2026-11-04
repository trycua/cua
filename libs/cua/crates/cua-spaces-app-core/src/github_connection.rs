// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings, Connections: the GitHub sign-in Cua uses when an agent asks to
//! push a branch and open a pull request.
//!
//! The daemon (`github.status`, `cua_spaces::agents::github_auth`) answers
//! what is signed in on each host: a token saved in Connections, `GH_TOKEN` in
//! the daemon's environment, or the GitHub CLI with one or several accounts.
//! This module only words it: one row per host with its state, and for each
//! host the page behind the row (the account picker, the switch, how Cua signs
//! in). No token ever reaches it: the input has names, accounts and scopes.
//!
//! The words of a host's state ("Signed in as ada with gh") are the daemon's,
//! the same ones the approval card and `cua connections github` use; every
//! label here is the core's.

use serde::{Deserialize, Serialize};

/// What `github.status` answered (camelCase here).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GithubInput {
    pub hosts: Vec<HostInput>,
    /// The GitHub CLI was found on this computer.
    pub gh_found: bool,
    /// The GitHub CLI is older than the version that lists accounts (2.81).
    pub gh_old: bool,
    /// A pasted token and what you chose can be kept on this computer.
    pub can_save: bool,
    /// The status could not be read (no daemon).
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HostInput {
    pub host: String,
    pub enabled: bool,
    /// `saved`, `env`, `gh` or none.
    pub source: Option<String>,
    pub login: Option<String>,
    /// The daemon's sentence for the state.
    pub line: String,
    /// Something to fix: a chosen account gone, a token refused.
    pub problem: bool,
    /// The token can do more than a push and a pull request need.
    pub warning: Option<String>,
    pub accounts: Vec<AccountInput>,
    pub sources: Vec<SourceInput>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AccountInput {
    pub login: String,
    /// gh's own account for the host.
    pub active: bool,
    pub selected: bool,
    pub chosen: bool,
    pub valid: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SourceInput {
    pub id: String,
    pub used: bool,
    pub present: bool,
    pub line: String,
}

/// How a row's state reads (the page draws a quiet mark, never a colour wall).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tone {
    /// Signed in and nothing to say.
    Ok,
    /// Not signed in, or switched off.
    Quiet,
    /// Something to fix (a chosen account gone, a token refused).
    Attention,
}

/// One host's row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRow {
    pub host: String,
    /// "GitHub", or the Enterprise host.
    pub title: String,
    /// The state in plain words.
    pub line: String,
    pub tone: Tone,
    /// The warning, when the token can do more than push and pull requests need.
    pub warning: Option<String>,
}

/// An account in the picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubAccountRow {
    pub login: String,
    /// "gh's own account" for the one gh uses itself.
    pub tag: Option<String>,
    pub selected: bool,
    /// The account's sign-in no longer works: "Sign in again with gh auth login".
    pub note: Option<String>,
}

/// What Cua tries, in order, and which one it is using.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubSourceRow {
    pub id: String,
    pub line: String,
    /// "In use", for the one Cua uses.
    pub tag: Option<String>,
}

/// The page behind one host's row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubDetail {
    pub host: String,
    pub title: String,
    pub line: String,
    pub warning: Option<String>,
    pub switch_label: String,
    pub switch_help: String,
    pub enabled: bool,
    /// The picker: only when gh has more than one account on the host.
    pub accounts_label: Option<String>,
    pub accounts_help: Option<String>,
    pub accounts: Vec<GithubAccountRow>,
    /// "Use gh's own account", to undo a choice.
    pub auto_label: Option<String>,
    pub sources_title: String,
    pub sources: Vec<GithubSourceRow>,
    /// What to do when nothing is signed in.
    pub help: Option<String>,
    pub rescan_label: String,
    /// "Paste a token" (github.com only, when a token can be saved).
    pub paste_label: Option<String>,
    pub close_label: String,
}

/// The section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubView {
    pub title: String,
    pub footer: String,
    pub rows: Vec<GithubRow>,
    pub details: Vec<GithubDetail>,
    pub rescan_label: String,
    /// Why the status is missing (no daemon).
    pub notice: Option<String>,
}

const GITHUB: &str = "github.com";

fn title_of(host: &str) -> String {
    if host == GITHUB {
        "GitHub".into()
    } else {
        host.into()
    }
}

fn row(h: &HostInput) -> GithubRow {
    let tone = if h.problem && h.enabled {
        Tone::Attention
    } else if h.enabled && h.source.is_some() {
        Tone::Ok
    } else {
        Tone::Quiet
    };
    GithubRow {
        host: h.host.clone(),
        title: title_of(&h.host),
        line: h.line.clone(),
        tone,
        warning: h.warning.clone().filter(|_| h.enabled),
    }
}

fn detail(h: &HostInput, input: &GithubInput) -> GithubDetail {
    let accounts: Vec<GithubAccountRow> = h
        .accounts
        .iter()
        .map(|a| GithubAccountRow {
            login: a.login.clone(),
            tag: a.active.then(|| "gh's own account".to_string()),
            selected: a.selected,
            note: (!a.valid).then(|| "Sign in again with gh auth login".to_string()),
        })
        .collect();
    let many = accounts.len() > 1;
    let nothing = h.source.is_none() && h.enabled && !h.problem;
    let cmd = if h.host == GITHUB {
        "gh auth login".to_string()
    } else {
        format!("gh auth login --hostname {}", h.host)
    };
    let help = if nothing {
        Some(if !input.gh_found {
            let paste = if h.host == GITHUB && input.can_save {
                " or paste a token"
            } else {
                ""
            };
            format!("Install the GitHub CLI and run {cmd}{paste}, then Rescan.")
        } else {
            let paste = if h.host == GITHUB && input.can_save {
                ", or paste a token"
            } else {
                ""
            };
            format!("Run {cmd} in a terminal{paste}, then Rescan.")
        })
    } else if input.gh_old && h.host == GITHUB {
        Some("Update gh to 2.81 or later to choose between accounts.".to_string())
    } else {
        None
    };
    GithubDetail {
        host: h.host.clone(),
        title: title_of(&h.host),
        line: h.line.clone(),
        warning: h.warning.clone().filter(|_| h.enabled),
        switch_label: "Use for pushes and pull requests".into(),
        switch_help: format!(
            "Turn off and Cua uses no GitHub sign-in on {}. Agents can still ask; the answer is no.",
            h.host
        ),
        enabled: h.enabled,
        accounts_label: many.then(|| "Account".to_string()),
        accounts_help: many
            .then(|| "Which gh account pushes and pull requests use. Without a choice, gh's own account is used.".to_string()),
        auto_label: (many && h.accounts.iter().any(|a| a.chosen))
            .then(|| "Use gh's own account".to_string()),
        accounts: if many { accounts } else { vec![] },
        sources_title: "How Cua signs in".into(),
        sources: h
            .sources
            .iter()
            .map(|s| GithubSourceRow {
                id: s.id.clone(),
                line: s.line.clone(),
                tag: s.used.then(|| "In use".to_string()),
            })
            .collect(),
        help,
        rescan_label: "Rescan".into(),
        paste_label: (h.host == GITHUB && input.can_save).then(|| "Paste a token".to_string()),
        close_label: "Done".into(),
    }
}

/// The section for the daemon's answer.
pub fn view(input: &GithubInput) -> GithubView {
    GithubView {
        title: "GitHub".into(),
        footer: "Cua uses this to push a branch and open a pull request after you approve a card. The sign-in stays on this computer; Spaces never get it.".into(),
        rows: input.hosts.iter().map(row).collect(),
        details: input.hosts.iter().map(|h| detail(h, input)).collect(),
        rescan_label: "Rescan".into(),
        notice: input.error.clone().map(|e| {
            format!("The GitHub sign-in could not be read: {e}")
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, line: &str, source: Option<&str>) -> HostInput {
        HostInput {
            host: name.into(),
            enabled: true,
            source: source.map(String::from),
            line: line.into(),
            ..Default::default()
        }
    }

    fn input(hosts: Vec<HostInput>) -> GithubInput {
        GithubInput {
            hosts,
            gh_found: true,
            can_save: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_row_says_the_state_and_marks_only_what_needs_a_look() {
        let mut warned = host("github.com", "Signed in as ada with gh", Some("gh"));
        warned.warning = Some("This sign-in can also change workflows.".into());
        let v = view(&input(vec![
            host("github.com", "Signed in as ada with gh", Some("gh")),
            warned.clone(),
            host(
                "ghe.example.com",
                "No sign-in found for ghe.example.com: run gh auth login --hostname ghe.example.com",
                None,
            ),
        ]));
        assert_eq!(v.rows[0].title, "GitHub");
        assert_eq!(v.rows[0].tone, Tone::Ok);
        // A broad token is a mark, not a red line: nothing is broken.
        assert_eq!(v.rows[1].tone, Tone::Ok);
        assert!(v.rows[1].warning.is_some());
        assert_eq!(v.rows[2].title, "ghe.example.com");
        assert_eq!(v.rows[2].tone, Tone::Quiet);
        // A host switched off shows no warning.
        warned.enabled = false;
        assert!(row(&warned).warning.is_none());
        assert_eq!(row(&warned).tone, Tone::Quiet);
    }

    #[test]
    fn the_picker_appears_only_with_several_accounts_and_undoes_a_choice() {
        let mut h = host("github.com", "Signed in as bob with gh", Some("gh"));
        h.accounts = vec![
            AccountInput {
                login: "ada".into(),
                active: true,
                valid: true,
                ..Default::default()
            },
            AccountInput {
                login: "bob".into(),
                selected: true,
                chosen: true,
                valid: true,
                ..Default::default()
            },
        ];
        let d = detail(&h, &input(vec![h.clone()]));
        assert_eq!(d.accounts_label.as_deref(), Some("Account"));
        assert_eq!(d.accounts.len(), 2);
        assert_eq!(d.accounts[0].tag.as_deref(), Some("gh's own account"));
        assert!(d.accounts[1].selected);
        assert_eq!(d.auto_label.as_deref(), Some("Use gh's own account"));
        let mut one = h.clone();
        one.accounts.truncate(1);
        let d = detail(&one, &input(vec![one.clone()]));
        assert!(d.accounts_label.is_none() && d.accounts.is_empty() && d.auto_label.is_none());
    }

    #[test]
    fn nothing_signed_in_says_what_to_run() {
        let h = host(
            "github.com",
            "No GitHub sign-in found: run gh auth login or paste a token",
            None,
        );
        let d = detail(&h, &input(vec![h.clone()]));
        assert_eq!(
            d.help.as_deref(),
            Some("Run gh auth login in a terminal, or paste a token, then Rescan.")
        );
        assert_eq!(d.paste_label.as_deref(), Some("Paste a token"));
        let mut i = input(vec![h.clone()]);
        i.gh_found = false;
        i.can_save = false;
        let d = detail(&h, &i);
        assert_eq!(
            d.help.as_deref(),
            Some("Install the GitHub CLI and run gh auth login, then Rescan.")
        );
        assert!(d.paste_label.is_none());
        let e = host("ghe.example.com", "No sign-in found", None);
        let d = detail(&e, &input(vec![e.clone()]));
        assert_eq!(
            d.help.as_deref(),
            Some("Run gh auth login --hostname ghe.example.com in a terminal, then Rescan.")
        );
        assert!(d.paste_label.is_none(), "a pasted token is for github.com");
    }

    #[test]
    fn no_text_has_a_middle_dot_or_a_token() {
        let mut h = host("github.com", "Signed in as ada with gh", Some("gh"));
        h.sources = vec![SourceInput {
            id: "gh".into(),
            used: true,
            present: true,
            line: "gh, signed in as ada".into(),
        }];
        let v = view(&input(vec![h]));
        let text = serde_json::to_string(&v).unwrap();
        assert!(!text.contains('\u{b7}'));
        assert_eq!(v.details[0].sources[0].tag.as_deref(), Some("In use"));
        let e = view(&GithubInput {
            error: Some("no daemon".into()),
            ..Default::default()
        });
        assert_eq!(
            e.notice.as_deref(),
            Some("The GitHub sign-in could not be read: no daemon")
        );
    }
}
