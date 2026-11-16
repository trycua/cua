// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Tool-call rows in a thread: one muted line per call, grouped under one
//! phrase ("Ran 4 commands, read 2 files").
//!
//! The daemon writes each row (`cua_agents::tool_row::tool_row`: its category
//! and its one collapsed line). What a client adds is the group's phrase, and
//! that table lives here so every shell says the same words and a phone never
//! keeps a copy of its own. `cua-agents` is a dev-dependency of this crate
//! (a phone cannot link it), and the tests below hold [`group_label`] equal to
//! `cua_agents::tool_row::group_label` for every category and to the crate's
//! shared fixture, so the two cannot drift.

use serde::{Deserialize, Serialize};

/// One tool call as the daemon sends it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolCallInput {
    /// `ran`, `read`, `edited`, `searched`, `fetched`, `computer`, `planned`, `delegated`, `other`.
    pub cat: String,
    /// The collapsed line (the daemon's `ToolRow::line`).
    pub line: String,
    /// The short text under an opened row.
    pub detail: String,
    /// `running`, `done` or `failed`; empty is done.
    pub state: String,
}

/// One row of an opened group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRowView {
    pub line: String,
    pub detail: String,
    /// `running`, `done` or `failed`.
    pub state: String,
    /// "Running" or "Failed"; none when done.
    pub state_label: Option<String>,
}

/// A group of calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolGroupView {
    /// The group's phrase; none with no calls.
    pub phrase: Option<String>,
    pub rows: Vec<ToolRowView>,
    /// Some call is still running.
    pub running: bool,
    /// Some call failed.
    pub failed: bool,
    pub show_label: String,
    pub hide_label: String,
    /// The line read aloud for the closed group.
    pub closed_label: String,
}

fn phrase_of(cat: &str, n: usize) -> String {
    let (verb, one, many) = match cat {
        "ran" => ("ran", "command", "commands"),
        "read" => ("read", "file", "files"),
        "edited" => ("edited", "file", "files"),
        "searched" => ("searched", "time", "times"),
        "fetched" => ("fetched", "page", "pages"),
        "computer" => ("used", "machine action", "machine actions"),
        "planned" => ("updated", "plan", "plans"),
        "delegated" => ("delegated", "task", "tasks"),
        _ => ("used", "tool", "tools"),
    };
    match (cat, n) {
        ("searched", 1) => "searched once".into(),
        ("searched", _) => format!("searched {n} times"),
        (_, 1) => format!("{verb} 1 {one}"),
        _ => format!("{verb} {n} {many}"),
    }
}

/// The group header for calls of these categories, in order of first use:
/// `Ran 4 commands, read 2 files`, at most three clauses and then `N more`.
/// No calls: `None`. (`cua_agents::tool_row::group_label`'s words.)
pub fn group_label(cats: &[&str]) -> Option<String> {
    let mut order: Vec<(&str, usize)> = Vec::new();
    for c in cats {
        let known = if matches!(
            *c,
            "ran"
                | "read"
                | "edited"
                | "searched"
                | "fetched"
                | "computer"
                | "planned"
                | "delegated"
        ) {
            *c
        } else {
            "other"
        };
        match order.iter_mut().find(|(k, _)| *k == known) {
            Some((_, n)) => *n += 1,
            None => order.push((known, 1)),
        }
    }
    if order.is_empty() {
        return None;
    }
    let mut parts: Vec<String> = order
        .iter()
        .take(3)
        .map(|(c, n)| phrase_of(c, *n))
        .collect();
    if order.len() > 3 {
        let rest: usize = order.iter().skip(3).map(|(_, n)| n).sum();
        parts.push(format!("{rest} more"));
    }
    let mut s = parts.join(", ");
    if let Some(first) = s.chars().next() {
        let up: String = first.to_uppercase().collect();
        s.replace_range(0..first.len_utf8(), &up);
    }
    Some(s)
}

/// The group a thread draws for `calls`.
pub fn group_view(calls: &[ToolCallInput]) -> ToolGroupView {
    let cats: Vec<&str> = calls.iter().map(|c| c.cat.as_str()).collect();
    let phrase = group_label(&cats);
    let rows: Vec<ToolRowView> = calls
        .iter()
        .map(|c| {
            let state = match c.state.as_str() {
                "running" => "running",
                "failed" => "failed",
                _ => "done",
            };
            ToolRowView {
                line: c.line.clone(),
                detail: c.detail.clone(),
                state: state.into(),
                state_label: match state {
                    "running" => Some("Running".into()),
                    "failed" => Some("Failed".into()),
                    _ => None,
                },
            }
        })
        .collect();
    let running = rows.iter().any(|r| r.state == "running");
    let failed = rows.iter().any(|r| r.state == "failed");
    let name = phrase.clone().unwrap_or_else(|| "Tool calls".into());
    ToolGroupView {
        closed_label: format!("{name}. Show details"),
        phrase,
        rows,
        running,
        failed,
        show_label: "Show details".into(),
        hide_label: "Hide details".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_agents::tool_row::{Cat, group_label as agents_label};

    const ALL: [Cat; 9] = [
        Cat::Ran,
        Cat::Read,
        Cat::Edited,
        Cat::Searched,
        Cat::Fetched,
        Cat::Computer,
        Cat::Planned,
        Cat::Delegated,
        Cat::Other,
    ];

    #[test]
    fn every_category_and_count_reads_as_cua_agents_reads() {
        for cat in ALL {
            for n in 1..=4 {
                let theirs = agents_label(&vec![cat; n]);
                let ours = group_label(&vec![cat.word(); n]);
                assert_eq!(ours, theirs, "{} x{n}", cat.word());
            }
        }
    }

    #[test]
    fn mixed_groups_and_the_long_tail_read_as_cua_agents_reads() {
        let seqs: Vec<Vec<Cat>> = vec![
            ALL.to_vec(),
            vec![Cat::Ran, Cat::Read, Cat::Read, Cat::Edited],
            vec![
                Cat::Other,
                Cat::Computer,
                Cat::Computer,
                Cat::Searched,
                Cat::Fetched,
                Cat::Fetched,
            ],
            vec![],
        ];
        for s in seqs {
            let words: Vec<&str> = s.iter().map(|c| c.word()).collect();
            assert_eq!(group_label(&words), agents_label(&s));
        }
    }

    #[test]
    fn the_shared_fixture_cases_hold() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cua-agents/tests/fixtures/tool-rows.json"
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let groups = v["groups"].as_array().unwrap();
        assert!(!groups.is_empty());
        for g in groups {
            let cats: Vec<&str> = g["cats"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c.as_str().unwrap())
                .collect();
            let want = g["label"].as_str().map(str::to_string);
            assert_eq!(group_label(&cats), want, "{cats:?}");
        }
    }

    #[test]
    fn a_group_view_marks_running_and_failed_rows() {
        let calls = vec![
            ToolCallInput {
                cat: "ran".into(),
                line: "Ran pytest".into(),
                detail: "pytest -x".into(),
                state: "done".into(),
            },
            ToolCallInput {
                cat: "edited".into(),
                line: "Edited cart.py".into(),
                detail: String::new(),
                state: "running".into(),
            },
            ToolCallInput {
                cat: "ran".into(),
                line: "Ran make".into(),
                detail: String::new(),
                state: "failed".into(),
            },
        ];
        let v = group_view(&calls);
        assert_eq!(v.phrase.as_deref(), Some("Ran 2 commands, edited 1 file"));
        assert!(v.running && v.failed);
        assert_eq!(v.rows[1].state_label.as_deref(), Some("Running"));
        assert_eq!(v.rows[2].state_label.as_deref(), Some("Failed"));
        assert_eq!(v.rows[0].state_label, None);
        assert_eq!(
            v.closed_label,
            "Ran 2 commands, edited 1 file. Show details"
        );
        let none = group_view(&[]);
        assert_eq!(none.phrase, None);
        assert_eq!(none.closed_label, "Tool calls. Show details");
    }
}
