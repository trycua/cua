// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Every agent you work with, as one list: the Agents page and Home's "Your
//! agents" both draw it, so they never disagree.
//!
//! - **Agents in Spaces** (the threads' members): one row per agent across
//!   its Spaces, keyed on who owns it, its kind and its name; its status
//!   line (waiting for you, working on what, failed, stopped, asleep).
//! - **Runs that never became one** (this device's): a job whose Space is
//!   still being made, an @mention of an agent that isn't connected yet, a
//!   start that failed. Each keeps its job, its Space, and the action that
//!   moves it on (Connect, Try again).
//!
//! Order: what needs your Connect, what waits for you, what's working,
//! what's starting, then the most recent.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListMember {
    /// Its handle in the Space (`claude`).
    pub handle: String,
    pub harness: String,
    /// The name it chose (empty: the handle).
    pub display_name: String,
    /// Who owns it (empty: not known; then the row is per Space).
    pub owner_id: String,
    /// `working`, `claimed`, `idle`, `asleep`, `paused`, `failed`, `stopped`, ...
    pub status: String,
    pub activity: Option<String>,
    pub unread: u32,
    pub last_ms: u64,
    /// When each thing waiting for you was asked (newest decides the order).
    pub needs_ms: Vec<u64>,
    /// It asked to join and waits to be let in: not listed.
    pub pending: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListSpace {
    pub space_id: String,
    pub space_name: String,
    pub members: Vec<ListMember>,
}

/// A run of this device's that isn't an agent in a Space (yet).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ListAttempt {
    pub id: String,
    /// Its Space (a `pending:` id while it is being made).
    pub space_id: String,
    pub space_name: String,
    pub harness: String,
    /// The agent's name ("Claude Code").
    pub label: String,
    /// What it was asked to do.
    pub job: String,
    pub at_ms: u64,
    /// `creating`, `not-connected`, `failed`, `stopped`.
    pub state: String,
    /// Not connected: the thread's own words ("Claude Code isn't connected yet") and Connect's.
    pub note: Option<String>,
    pub connect_label: Option<String>,
    /// The Connectors app Connect runs for.
    pub app: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentListInput {
    pub spaces: Vec<ListSpace>,
    pub attempts: Vec<ListAttempt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAction {
    /// `connect`, `retry`.
    pub id: String,
    pub label: String,
}

/// Where a row's agent is: a Space and its handle there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPlace {
    pub space_id: String,
    pub space_name: String,
    pub handle: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentListRow {
    pub key: String,
    /// `agent` (in a Space) or `attempt` (this device's run).
    pub kind: String,
    pub label: String,
    pub harness: String,
    /// `you`, `not-connected`, `working`, `creating`, `failed`, `stopped`, `idle`, `asleep`.
    pub tone: String,
    pub line: String,
    /// What it was asked to do (a run's job; an agent's current work).
    pub job: Option<String>,
    pub last_ms: u64,
    pub unread: u32,
    pub places: Vec<ListPlace>,
    pub action: Option<ListAction>,
    pub app: Option<String>,
    /// For an attempt: its id.
    pub attempt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentListView {
    pub rows: Vec<AgentListRow>,
    /// Waiting for you, working or starting.
    pub live: usize,
    /// "2 working" (absent with none).
    pub summary: Option<String>,
    pub title: String,
    /// "See all 5".
    pub see_all_label: String,
}

/// One agent across Spaces: the same owner, kind and name. Without an owner, its handle in its Space.
pub fn member_key(space_id: &str, m: &ListMember) -> String {
    if m.owner_id.is_empty() {
        return format!("{space_id}/{}", m.handle);
    }
    let kind = if m.harness.is_empty() {
        "other"
    } else {
        &m.harness
    };
    let name = if m.display_name.is_empty() {
        &m.handle
    } else {
        &m.display_name
    };
    format!("{}/{kind}/{}", m.owner_id, name.trim().to_lowercase())
}

/// "bumping the retry timeout in ubuntu-build" reads "Bumping the retry timeout".
fn sentence(s: &str) -> String {
    let t = s.trim();
    let t = match t.rfind(" in ") {
        Some(i) if !t[i + 4..].contains(' ') => &t[..i],
        _ => t,
    };
    let t = if t.eq_ignore_ascii_case("waiting for you") {
        "Waiting"
    } else {
        t
    };
    let mut c = t.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

const WORKING: &[&str] = &["working", "claimed"];

pub fn view(input: &AgentListInput) -> AgentListView {
    struct Acc {
        row: AgentListRow,
        statuses: Vec<(String, String, Option<String>)>,
        needs: Vec<(u64, String)>,
    }
    let mut order: Vec<String> = vec![];
    let mut by: std::collections::HashMap<String, Acc> = Default::default();
    for sp in &input.spaces {
        for m in sp.members.iter().filter(|m| !m.pending) {
            let key = member_key(&sp.space_id, m);
            let acc = by.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                Acc {
                    row: AgentListRow {
                        key: key.clone(),
                        kind: "agent".into(),
                        label: if m.display_name.is_empty() {
                            m.handle.clone()
                        } else {
                            m.display_name.clone()
                        },
                        harness: m.harness.clone(),
                        tone: "idle".into(),
                        line: String::new(),
                        job: None,
                        last_ms: 0,
                        unread: 0,
                        places: vec![],
                        action: None,
                        app: None,
                        attempt: None,
                    },
                    statuses: vec![],
                    needs: vec![],
                }
            });
            acc.row.places.push(ListPlace {
                space_id: sp.space_id.clone(),
                space_name: sp.space_name.clone(),
                handle: m.handle.clone(),
            });
            acc.row.unread += m.unread;
            acc.row.last_ms = acc.row.last_ms.max(m.last_ms);
            acc.needs
                .extend(m.needs_ms.iter().map(|&t| (t, sp.space_name.clone())));
            acc.statuses
                .push((m.status.clone(), sp.space_name.clone(), m.activity.clone()));
        }
    }
    let mut rows: Vec<AgentListRow> = order
        .into_iter()
        .filter_map(|k| by.remove(&k))
        .map(|mut a| {
            let newest = a.needs.iter().max_by_key(|(t, _)| *t).cloned();
            if let Some((t, _)) = &newest {
                a.row.last_ms = a.row.last_ms.max(*t);
            }
            let r = &mut a.row;
            let working = a
                .statuses
                .iter()
                .find(|(s, ..)| WORKING.contains(&s.as_str()));
            let failed = a.statuses.iter().find(|(s, ..)| s == "failed");
            let stopped = a.statuses.iter().find(|(s, ..)| s == "stopped");
            let in_spaces = |n: usize| {
                if n > 1 {
                    format!("{n} Spaces")
                } else {
                    String::new()
                }
            };
            if let Some((_, space)) = newest {
                r.tone = "you".into();
                // The newest ask says where.
                r.line = format!("Waiting for you in {space}");
            } else if let Some((_, space, activity)) = working {
                r.tone = "working".into();
                let what = activity.as_deref().map(sentence).filter(|s| !s.is_empty());
                r.job = activity.clone().filter(|s| !s.is_empty());
                r.line = format!("{} in {space}", what.unwrap_or_else(|| "Working".into()));
            } else if let Some((_, space, _)) = failed {
                r.tone = "failed".into();
                r.line = format!("Failed in {space}");
            } else if let Some((_, space, _)) = stopped {
                r.tone = "stopped".into();
                r.line = format!("Stopped in {space}");
            } else if a
                .statuses
                .iter()
                .all(|(s, ..)| s == "asleep" || s == "paused")
            {
                r.tone = "asleep".into();
                r.line = if r.places.len() > 1 {
                    format!("Asleep in {}", in_spaces(r.places.len()))
                } else {
                    format!("Asleep in {}", r.places[0].space_name)
                };
            } else {
                r.tone = "idle".into();
                r.line = if r.places.len() > 1 {
                    format!("In {}", in_spaces(r.places.len()))
                } else {
                    format!("In {}", r.places[0].space_name)
                };
            }
            a.row
        })
        .collect();
    rows.extend(input.attempts.iter().map(|t| {
        let (tone, line, action) = match t.state.as_str() {
            "creating" => (
                "creating",
                format!("Waiting for {} to be ready", t.space_name),
                None,
            ),
            "not-connected" => (
                "not-connected",
                t.note
                    .clone()
                    .unwrap_or_else(|| format!("{} isn't connected yet", t.label)),
                t.connect_label.clone().map(|label| ListAction {
                    id: "connect".into(),
                    label,
                }),
            ),
            "stopped" => ("stopped", format!("Stopped in {}", t.space_name), None),
            _ => (
                "failed",
                if t.space_name.is_empty() {
                    "Didn't start".to_string()
                } else {
                    format!("Didn't start in {}", t.space_name)
                },
                Some(ListAction {
                    id: "retry".into(),
                    label: "Try again".into(),
                }),
            ),
        };
        AgentListRow {
            key: format!("attempt:{}", t.id),
            kind: "attempt".into(),
            label: t.label.clone(),
            harness: t.harness.clone(),
            tone: tone.into(),
            line,
            job: Some(t.job.clone()).filter(|j| !j.is_empty()),
            last_ms: t.at_ms,
            unread: 0,
            places: if t.space_id.is_empty() {
                vec![]
            } else {
                vec![ListPlace {
                    space_id: t.space_id.clone(),
                    space_name: t.space_name.clone(),
                    handle: String::new(),
                }]
            },
            action,
            app: t.app.clone(),
            attempt: Some(t.id.clone()),
        }
    }));
    let rank = |r: &AgentListRow| match r.tone.as_str() {
        // A run of yours that can't start needs you most: it is waiting on your Connect.
        "not-connected" => 0,
        "you" => 1,
        "working" => 2,
        "creating" => 3,
        _ => 4,
    };
    rows.sort_by(|a, b| {
        rank(a)
            .cmp(&rank(b))
            .then(b.last_ms.cmp(&a.last_ms))
            .then(a.label.cmp(&b.label))
    });
    let working = rows.iter().filter(|r| r.tone == "working").count();
    let live = rows
        .iter()
        .filter(|r| matches!(r.tone.as_str(), "you" | "working" | "creating"))
        .count();
    AgentListView {
        summary: (working > 0).then(|| format!("{working} working")),
        live,
        title: "Your agents".into(),
        see_all_label: format!("See all {}", rows.len()),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(handle: &str, status: &str) -> ListMember {
        ListMember {
            handle: handle.into(),
            harness: "hermes".into(),
            display_name: "Hermes".into(),
            owner_id: "acct_ada".into(),
            status: status.into(),
            ..Default::default()
        }
    }

    fn attempt(id: &str, state: &str) -> ListAttempt {
        ListAttempt {
            id: id.into(),
            space_id: format!("pending:{id}"),
            space_name: "space-1".into(),
            harness: "claude-code".into(),
            label: "Claude Code".into(),
            job: "fix the build".into(),
            at_ms: 10,
            state: state.into(),
            note: (state == "not-connected").then(|| "Claude Code isn't connected yet".into()),
            connect_label: (state == "not-connected").then(|| "Connect Claude Code".into()),
            app: Some("claude-code".into()),
        }
    }

    #[test]
    fn a_cant_wake_run_and_a_creating_run_are_in_the_one_list() {
        let v = view(&AgentListInput {
            spaces: vec![ListSpace {
                space_id: "relay:a/b".into(),
                space_name: "ubuntu-build".into(),
                members: vec![ListMember {
                    activity: Some("bumping the retry timeout".into()),
                    last_ms: 5,
                    ..member("hermes", "working")
                }],
            }],
            attempts: vec![attempt("a", "not-connected"), attempt("b", "creating")],
        });
        let tones: Vec<_> = v.rows.iter().map(|r| r.tone.as_str()).collect();
        assert_eq!(tones, ["not-connected", "working", "creating"]);
        let nc = &v.rows[0];
        assert_eq!(nc.kind, "attempt");
        assert_eq!(nc.line, "Claude Code isn't connected yet");
        assert_eq!(nc.action.as_ref().unwrap().label, "Connect Claude Code");
        assert_eq!(nc.job.as_deref(), Some("fix the build"));
        assert_eq!(nc.places[0].space_id, "pending:a");
        assert_eq!(v.rows[1].line, "Bumping the retry timeout in ubuntu-build");
        assert_eq!(v.rows[2].line, "Waiting for space-1 to be ready");
        assert_eq!(v.summary.as_deref(), Some("1 working"));
        assert_eq!(v.live, 2);
        assert_eq!(v.see_all_label, "See all 3");
    }

    #[test]
    fn failed_and_stopped_runs_keep_their_space_and_retry() {
        let v = view(&AgentListInput {
            spaces: vec![ListSpace {
                space_id: "s".into(),
                space_name: "web".into(),
                members: vec![
                    member("hermes", "failed"),
                    ListMember {
                        owner_id: "x".into(),
                        ..member("codex", "stopped")
                    },
                ],
            }],
            attempts: vec![attempt("c", "failed")],
        });
        let lines: Vec<_> = v.rows.iter().map(|r| r.line.as_str()).collect();
        assert!(lines.contains(&"Failed in web"));
        assert!(lines.contains(&"Stopped in web"));
        assert!(lines.contains(&"Didn't start in space-1"));
        let retry = v.rows.iter().find(|r| r.kind == "attempt").unwrap();
        assert_eq!(retry.action.as_ref().unwrap().id, "retry");
    }

    #[test]
    fn one_agent_in_two_spaces_is_one_row_and_waiting_for_you_comes_first() {
        let v = view(&AgentListInput {
            spaces: vec![
                ListSpace {
                    space_id: "s1".into(),
                    space_name: "one".into(),
                    members: vec![member("hermes", "asleep")],
                },
                ListSpace {
                    space_id: "s2".into(),
                    space_name: "two".into(),
                    members: vec![
                        member("hermes", "asleep"),
                        ListMember {
                            owner_id: "acct_grace".into(),
                            display_name: "Claude Code".into(),
                            needs_ms: vec![99],
                            ..member("claude", "idle")
                        },
                    ],
                },
            ],
            attempts: vec![],
        });
        assert_eq!(v.rows.len(), 2);
        assert_eq!(v.rows[0].tone, "you");
        assert_eq!(v.rows[0].line, "Waiting for you in two");
        assert_eq!(v.rows[1].line, "Asleep in 2 Spaces");
        assert_eq!(v.rows[1].places.len(), 2);
        // Without an owner, the row is per Space.
        assert_eq!(
            member_key(
                "s1",
                &ListMember {
                    owner_id: String::new(),
                    ..member("x", "idle")
                }
            ),
            "s1/x"
        );
    }
}
