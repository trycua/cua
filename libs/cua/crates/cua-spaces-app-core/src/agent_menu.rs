// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Which coding apps the agent menu of Home shows (Settings, Connections). One rule for the web, SwiftUI and Electron apps.
//!
//! By default Codex, Hermes, Claude Code and OpenCode are shown, and any other
//! app only while one of its accounts works (today's menu). The person can
//! hide a leading app or show another one; the choice is kept in
//! `AppSettings.agent_menu_shown` and `agent_menu_hidden` as the apps that
//! differ from the default, so a new default still reaches everyone who did
//! not choose. Hiding changes only the menu: the app stays on the Agents page
//! and answers to its name (@mention, CLI, MCP), and its sign-ins are kept.
//!
//! The menu is never empty: the last app shown cannot be hidden, and a choice
//! that would leave nothing (an app signed out since) is read as the defaults.

use serde::{Deserialize, Serialize};

use crate::connectors::{self, AgentGroup, ConnectorsInput, MENU_FIRST};

/// The apps whose default the person changed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentMenuPrefs {
    /// Apps shown although the default hides them (an app outside the leading four, not signed in).
    pub shown: Vec<String>,
    /// Apps hidden although the default shows them.
    pub hidden: Vec<String>,
    /// The working accounts last seen, as `app|variable|slot`: a new one is a connect, which brings its
    /// app back into the menu ([`sync`]). Nothing secret.
    pub seen: Vec<String>,
}

/// The working accounts of every app, as `app|variable|slot`.
fn account_ids(input: &ConnectorsInput) -> Vec<String> {
    let mut out = vec![];
    for g in connectors::menu_groups(input) {
        for a in &g.accounts {
            out.push(format!(
                "{}|{}|{}",
                g.app,
                a.env,
                a.slot.as_deref().unwrap_or("")
            ));
        }
    }
    out
}

/// The choice after the accounts changed: an app that is hidden and gained a working account (signed in,
/// a key added, a sign-in that ran out renewed, from the menu, Connections, onboarding or the CLI) is
/// shown again. An app that was already connected stays as it is, and an account taken away changes
/// nothing. Run it whenever the accounts are read; the result is what to keep.
pub fn sync(input: &ConnectorsInput, prefs: &AgentMenuPrefs) -> AgentMenuPrefs {
    let now = account_ids(input);
    let mut next = prefs.clone();
    next.hidden.retain(|app| {
        let prefix = format!("{app}|");
        !now.iter()
            .any(|id| id.starts_with(&prefix) && !prefs.seen.contains(id))
    });
    next.seen = now;
    next
}

/// Whether the default shows `group`: the leading four always, the others while an account works.
fn default_shown(group: &AgentGroup) -> bool {
    MENU_FIRST.contains(&group.app.as_str()) || group.state != "not-connected"
}

fn chosen(prefs: &AgentMenuPrefs, group: &AgentGroup) -> bool {
    if prefs.hidden.contains(&group.app) {
        false
    } else if prefs.shown.contains(&group.app) {
        true
    } else {
        default_shown(group)
    }
}

/// The apps (setup ids) the menu shows, in menu order. Never empty while there is an app: when the
/// choice would leave none, the defaults apply.
pub fn shown_apps(prefs: &AgentMenuPrefs, groups: &[AgentGroup]) -> Vec<String> {
    let picked: Vec<String> = groups
        .iter()
        .filter(|g| chosen(prefs, g))
        .map(|g| g.app.clone())
        .collect();
    if !picked.is_empty() {
        return picked;
    }
    groups
        .iter()
        .filter(|g| default_shown(g))
        .map(|g| g.app.clone())
        .collect()
}

/// One row of Settings, Connections, Agent menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMenuRow {
    /// The setup id (`codex`).
    pub app: String,
    pub label: String,
    /// The app is in the agent menu.
    pub shown: bool,
    /// What the app's sign-in is: "Signed in", "Sign in again" or "Not signed in".
    pub status: String,
    /// The switch cannot be turned off: this is the last app shown.
    pub locked: bool,
}

/// Settings, Connections, Agent menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMenuSettings {
    pub title: String,
    /// The switch's name on every row.
    pub switch_label: String,
    /// What hiding does, in a line.
    pub help: String,
    pub rows: Vec<AgentMenuRow>,
    /// Said while one app is left: why its switch is off limits.
    pub last_note: Option<String>,
}

const SWITCH: &str = "Show in the agent menu";

fn status(group: &AgentGroup) -> &'static str {
    match group.state.as_str() {
        "connected" => "Signed in",
        "needs-reconnect" => "Sign in again",
        _ => "Not signed in",
    }
}

/// The rows of Settings, Connections, Agent menu: every coding app, the leading four first.
pub fn settings_view(input: &ConnectorsInput, prefs: &AgentMenuPrefs) -> AgentMenuSettings {
    let all = connectors::menu_groups(input);
    let shown = shown_apps(prefs, &all);
    let only = shown.len() == 1;
    let last = shown.first().and_then(|a| all.iter().find(|g| &g.app == a));
    AgentMenuSettings {
        title: "Agent menu".into(),
        switch_label: SWITCH.into(),
        help: "Hidden apps keep their sign-ins.".into(),
        rows: all
            .iter()
            .map(|g| {
                let on = shown.contains(&g.app);
                AgentMenuRow {
                    app: g.app.clone(),
                    label: g.label.clone(),
                    shown: on,
                    status: status(g).into(),
                    locked: on && only,
                }
            })
            .collect(),
        last_note: only
            .then(|| {
                last.map(|g| {
                    format!(
                        "{} stays in the menu: at least one coding app has to be shown.",
                        g.label
                    )
                })
            })
            .flatten(),
    }
}

/// The result of a switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMenuChange {
    /// The choice to keep (unchanged when refused).
    pub prefs: AgentMenuPrefs,
    /// Said when the switch was refused: the last app cannot be hidden.
    pub refused: Option<String>,
}

/// The choice after `app` is switched on or off. Hiding the last app shown is refused; an unknown app
/// changes nothing. The result holds only what differs from the default.
pub fn choose(
    input: &ConnectorsInput,
    prefs: &AgentMenuPrefs,
    app: &str,
    on: bool,
) -> AgentMenuChange {
    let all = connectors::menu_groups(input);
    let keep = |refused: Option<String>| AgentMenuChange {
        prefs: prefs.clone(),
        refused,
    };
    let Some(group) = all.iter().find(|g| g.app == app) else {
        return keep(None);
    };
    let shown = shown_apps(prefs, &all);
    if !on && shown.contains(&group.app) && shown.len() == 1 {
        return keep(Some(format!(
            "{} stays in the menu: at least one coding app has to be shown.",
            group.label
        )));
    }
    let mut next = prefs.clone();
    // What is connected now is the baseline: only an account added after this is a new connect.
    next.seen = account_ids(input);
    next.shown.retain(|a| a != app);
    next.hidden.retain(|a| a != app);
    if on != default_shown(group) {
        if on {
            next.shown.push(app.into());
        } else {
            next.hidden.push(app.into());
        }
    }
    // A choice for an app that is gone is kept out of the file.
    next.shown.retain(|a| all.iter().any(|g| &g.app == a));
    next.hidden.retain(|a| all.iter().any(|g| &g.app == a));
    AgentMenuChange {
        prefs: next,
        refused: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_keys::{AgentKeyInput, AgentKeysInput};
    use crate::connectors::{agents_view, agents_view_with};

    fn input_with(envs: &[&str]) -> ConnectorsInput {
        ConnectorsInput {
            keys: AgentKeysInput {
                keys: envs
                    .iter()
                    .map(|env| AgentKeyInput {
                        provider: "anthropic".into(),
                        env: (*env).into(),
                        last4: "4f2a".into(),
                        added_ms: 1_000,
                        expired: false,
                        label: None,
                        region: None,
                        fields: Default::default(),
                    })
                    .collect(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn apps(i: &ConnectorsInput, p: &AgentMenuPrefs) -> Vec<String> {
        agents_view_with(i, p)
            .groups
            .into_iter()
            .map(|g| g.app)
            .collect()
    }

    #[test]
    fn the_default_is_todays_menu() {
        let none = input_with(&[]);
        assert_eq!(
            apps(&none, &AgentMenuPrefs::default()),
            ["codex", "hermes", "claude-code", "opencode"]
        );
        assert_eq!(
            agents_view(&none),
            agents_view_with(&none, &AgentMenuPrefs::default())
        );
    }

    #[test]
    fn another_app_is_shown_while_an_account_works() {
        let i = input_with(&["ANTHROPIC_API_KEY"]);
        let a = apps(&i, &AgentMenuPrefs::default());
        assert!(a.contains(&"goose".to_string()), "{a:?}");
        // Hiding it keeps its key.
        let hidden = choose(&i, &AgentMenuPrefs::default(), "goose", false);
        assert!(!apps(&i, &hidden.prefs).contains(&"goose".to_string()));
        assert_eq!(hidden.prefs.hidden, ["goose"]);
    }

    #[test]
    fn hiding_leaves_the_app_out_and_showing_brings_it_back() {
        let i = input_with(&[]);
        let hidden = choose(&i, &AgentMenuPrefs::default(), "hermes", false);
        assert!(hidden.refused.is_none());
        assert_eq!(hidden.prefs.hidden, ["hermes"]);
        assert_eq!(
            apps(&i, &hidden.prefs),
            ["codex", "claude-code", "opencode"]
        );
        let back = choose(&i, &hidden.prefs, "hermes", true);
        assert_eq!(
            back.prefs,
            AgentMenuPrefs::default(),
            "back to the default keeps nothing"
        );
    }

    #[test]
    fn an_app_outside_the_four_can_be_shown_before_it_is_signed_in() {
        let i = input_with(&[]);
        let on = choose(&i, &AgentMenuPrefs::default(), "goose", true);
        assert_eq!(on.prefs.shown, ["goose"]);
        assert!(apps(&i, &on.prefs).contains(&"goose".to_string()));
        let off = choose(&i, &on.prefs, "goose", false);
        assert_eq!(off.prefs, AgentMenuPrefs::default());
    }

    #[test]
    fn the_last_app_cannot_be_hidden() {
        let i = input_with(&[]);
        let mut p = AgentMenuPrefs::default();
        for app in ["codex", "hermes", "claude-code"] {
            let c = choose(&i, &p, app, false);
            assert!(c.refused.is_none(), "{app}");
            p = c.prefs;
        }
        assert_eq!(apps(&i, &p), ["opencode"]);
        let v = settings_view(&i, &p);
        assert_eq!(
            v.last_note.as_deref(),
            Some("OpenCode stays in the menu: at least one coding app has to be shown.")
        );
        assert!(v.rows.iter().find(|r| r.app == "opencode").unwrap().locked);
        let refused = choose(&i, &p, "opencode", false);
        assert_eq!(refused.prefs, p);
        assert!(refused.refused.is_some());
        // Another app on lets the last one go.
        let more = choose(&i, &p, "goose", true).prefs;
        assert!(choose(&i, &more, "opencode", false).refused.is_none());
    }

    #[test]
    fn a_file_that_hides_everything_reads_as_the_defaults() {
        let i = input_with(&[]);
        let p = AgentMenuPrefs {
            shown: vec![],
            hidden: ["codex", "hermes", "claude-code", "opencode"]
                .map(String::from)
                .to_vec(),
            seen: vec![],
        };
        assert_eq!(apps(&i, &p), ["codex", "hermes", "claude-code", "opencode"]);
    }

    #[test]
    fn rows_list_every_app_with_the_four_first() {
        let i = input_with(&[]);
        let v = settings_view(&i, &AgentMenuPrefs::default());
        let apps: Vec<&str> = v.rows.iter().map(|r| r.app.as_str()).collect();
        assert_eq!(&apps[..4], ["codex", "hermes", "claude-code", "opencode"]);
        assert_eq!(apps.len(), connectors::HARNESS_APPS.len());
        assert!(v.rows[..4].iter().all(|r| r.shown && !r.locked));
        assert!(v.rows[4..].iter().all(|r| !r.shown));
        assert!(v.last_note.is_none());
        assert_eq!(v.switch_label, "Show in the agent menu");
    }

    fn key_for(env: &str, label: Option<&str>) -> AgentKeyInput {
        AgentKeyInput {
            provider: "anthropic".into(),
            env: env.into(),
            last4: String::new(),
            added_ms: 1_000,
            expired: false,
            label: label.map(String::from),
            region: None,
            fields: Default::default(),
        }
    }

    fn with_claude(slots: &[(&str, &str)]) -> ConnectorsInput {
        let mut i = input_with(&[]);
        let mut it = slots.iter();
        if let Some((_, label)) = it.next() {
            i.keys
                .keys
                .push(key_for("CLAUDE_CODE_OAUTH_TOKEN", Some(label)));
        }
        for (slot, label) in it {
            i.key_extras.push(connectors::ExtraKeyInput {
                key: key_for("CLAUDE_CODE_OAUTH_TOKEN", Some(label)),
                slot: Some((*slot).into()),
            });
        }
        i
    }

    #[test]
    fn connecting_a_hidden_app_shows_it_again() {
        let none = with_claude(&[]);
        let hid = choose(&none, &AgentMenuPrefs::default(), "claude-code", false).prefs;
        assert!(!apps(&none, &hid).contains(&"claude-code".to_string()));
        // Nothing changes (a refresh, a relaunch): it stays hidden.
        assert_eq!(sync(&none, &hid), hid);
        // An account is added: shown again, and the choice no longer holds it.
        let one = with_claude(&[("", "a")]);
        let back = sync(&one, &hid);
        assert!(back.hidden.is_empty());
        assert!(apps(&one, &back).contains(&"claude-code".to_string()));
    }

    #[test]
    fn a_hidden_app_that_was_connected_stays_hidden_when_nothing_is_added() {
        let two = with_claude(&[("", "a"), ("w", "b")]);
        let hid = choose(&two, &AgentMenuPrefs::default(), "claude-code", false).prefs;
        assert_eq!(
            sync(&two, &hid),
            hid,
            "a refresh or relaunch changes nothing"
        );
        // One of two accounts is removed: still hidden.
        let one = with_claude(&[("", "a")]);
        let after = sync(&one, &hid);
        assert_eq!(after.hidden, ["claude-code"]);
        assert!(!apps(&one, &after).contains(&"claude-code".to_string()));
        // A new account after that brings it back.
        let three = with_claude(&[("", "a"), ("x", "c")]);
        assert!(sync(&three, &after).hidden.is_empty());
    }

    #[test]
    fn the_first_connect_of_an_app_hidden_by_default_shows_it() {
        let none = input_with(&[]);
        // Goose is not shown until an account works; the person also hid it.
        let hid = choose(&none, &AgentMenuPrefs::default(), "goose", true).prefs;
        let hid = choose(&none, &hid, "goose", false).prefs;
        assert_eq!(hid, AgentMenuPrefs::default());
        let key = input_with(&["ANTHROPIC_API_KEY"]);
        let p = sync(&key, &hid);
        assert!(apps(&key, &p).contains(&"goose".to_string()));
        // Hidden after it is connected, then synced again: it stays hidden.
        let h2 = choose(&key, &p, "goose", false).prefs;
        assert_eq!(sync(&key, &h2), h2);
        assert!(!apps(&key, &h2).contains(&"goose".to_string()));
    }

    #[test]
    fn an_account_renewed_after_it_ran_out_counts_as_a_connect() {
        let mut gone = with_claude(&[("", "a")]);
        gone.keys.keys[0].expired = true;
        let hid = choose(&gone, &AgentMenuPrefs::default(), "claude-code", false).prefs;
        assert_eq!(sync(&gone, &hid), hid);
        let renewed = with_claude(&[("", "a")]);
        assert!(sync(&renewed, &hid).hidden.is_empty());
    }

    #[test]
    fn an_unknown_app_changes_nothing() {
        let i = input_with(&[]);
        let c = choose(&i, &AgentMenuPrefs::default(), "nope", false);
        assert_eq!(c.prefs, AgentMenuPrefs::default());
        assert!(c.refused.is_none());
    }
}
