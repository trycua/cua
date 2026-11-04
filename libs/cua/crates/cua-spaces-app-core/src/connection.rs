// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The agents on This Mac, as rows: what each one's connection row says
//! and its one button, from the connection status every surface shares
//! (`agents.status`, `cua_agent_setup::connection`, CONTRACT §4). The
//! Agents page and the first run draw these words, so the app, the CLI and
//! MCP never disagree about whether an agent is connected.

use serde::{Deserialize, Serialize};

use crate::first_run::FirstRunButton;

/// A problem as the status reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionProblem {
    pub line: String,
    /// `reconnect`, `sign_in`, `install`, `disconnect`, `none`.
    pub action: String,
}

/// One `agents.status` row (snake_case, as the status serializes it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionInput {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    pub previous_version: Option<String>,
    /// `yes`, `no`, `unknown`, `expired`.
    pub signed_in: String,
    /// "ChatGPT sign-in".
    pub sign_in: Option<String>,
    /// `ok`, `missing`, `broken`, `stale`, `unsupported`.
    pub mcp: String,
    pub mcp_config: Option<String>,
    pub signin_imported: bool,
    /// `ok`, `expired`, `missing`, `unsupported`.
    pub signin_import: String,
    /// `connected`, `attention`, `not_connected`, `not_installed`.
    pub state: String,
    pub problem: Option<ConnectionProblem>,
    pub checked_at_ms: u64,
}

/// A row as drawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRowView {
    pub id: String,
    pub name: String,
    /// `connected`, `attention`, `not_connected`.
    pub state: String,
    /// "Connected", "Not connected", or the problem's one line.
    pub label: String,
    /// The label is a problem (drawn as one).
    pub problem: bool,
    /// Quiet facts under the name: "Codex 0.61.0, updated from 0.60.1".
    pub detail: Option<String>,
    /// "ChatGPT sign-in in Keyvault".
    pub sign_in: Option<String>,
    /// The one button: `connect`, `reconnect`, `sign-in`, `disconnect`,
    /// `install`.
    pub action: Option<FirstRunButton>,
}

fn button(id: &str, label: &str) -> FirstRunButton {
    FirstRunButton {
        id: id.into(),
        label: label.into(),
    }
}

/// One row; none for an agent that isn't on This Mac and has nothing of
/// Cua's left to take out.
pub fn connection_row(r: &ConnectionInput) -> Option<ConnectionRowView> {
    if r.state == "not_installed" {
        return None;
    }
    let detail = match (&r.version, &r.previous_version) {
        (Some(v), Some(p)) if p != v => Some(format!("{v}, updated from {p}")),
        (Some(v), _) => Some(v.clone()),
        _ => None,
    };
    let sign_in = match (r.signin_import.as_str(), &r.sign_in) {
        ("ok", Some(s)) => Some(format!("{s} in Keyvault")),
        _ => None,
    };
    let action_for = |p: &ConnectionProblem| match p.action.as_str() {
        "reconnect" => Some(button("reconnect", "Reconnect")),
        "sign_in" => Some(button("sign-in", "Sign in")),
        "disconnect" => Some(button("disconnect", "Disconnect")),
        "install" => Some(button("install", "Install")),
        _ => None,
    };
    let (label, problem, action) = match (r.state.as_str(), &r.problem) {
        ("connected", None) => ("Connected".to_string(), false, None),
        // Works, with a quiet note (another copy of cua).
        ("connected", Some(p)) => ("Connected".to_string(), false, action_for(p)),
        ("attention", Some(p)) => (p.line.clone(), true, action_for(p)),
        ("attention", None) => ("Something needs you.".to_string(), true, None),
        _ => (
            "Not connected".to_string(),
            false,
            Some(button("connect", "Connect")),
        ),
    };
    Some(ConnectionRowView {
        id: r.id.clone(),
        name: r.name.clone(),
        state: if r.state == "attention" || r.state == "connected" {
            r.state.clone()
        } else {
            "not_connected".into()
        },
        label,
        problem,
        detail,
        sign_in,
        action,
    })
}

/// Every row worth showing, in the status's order.
pub fn connection_rows(rows: &[ConnectionInput]) -> Vec<ConnectionRowView> {
    rows.iter().filter_map(connection_row).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(state: &str, problem: Option<(&str, &str)>) -> ConnectionInput {
        ConnectionInput {
            id: "codex".into(),
            name: "Codex".into(),
            installed: true,
            version: Some("0.61.0".into()),
            state: state.into(),
            problem: problem.map(|(l, a)| ConnectionProblem {
                line: l.into(),
                action: a.into(),
            }),
            ..Default::default()
        }
    }

    #[test]
    fn each_state_has_one_label_and_at_most_one_button() {
        let v = connection_row(&row("connected", None)).unwrap();
        assert_eq!(
            (v.label.as_str(), v.problem, v.action),
            ("Connected", false, None)
        );
        let v = connection_row(&row("not_connected", None)).unwrap();
        assert_eq!(v.label, "Not connected");
        assert_eq!(v.action.unwrap().id, "connect");
        let v = connection_row(&row(
            "attention",
            Some((
                "Cua was taken out of ~/.codex/config.toml. Reconnect adds it back.",
                "reconnect",
            )),
        ))
        .unwrap();
        assert!(v.problem);
        assert_eq!(v.action.unwrap().label, "Reconnect");
        let v = connection_row(&row(
            "attention",
            Some(("Connected. No ChatGPT sign-in on This Mac yet.", "sign_in")),
        ))
        .unwrap();
        assert_eq!(v.action.unwrap().id, "sign-in");
        let v = connection_row(&row(
            "attention",
            Some(("Connected. No provider key on This Mac yet.", "none")),
        ))
        .unwrap();
        assert!(v.problem && v.action.is_none());
        let v = connection_row(&row(
            "connected",
            Some((
                "Codex uses another copy of cua. Reconnect to use this one.",
                "reconnect",
            )),
        ))
        .unwrap();
        assert_eq!((v.label.as_str(), v.problem), ("Connected", false));
        assert_eq!(v.action.unwrap().id, "reconnect");
        assert!(connection_row(&row("not_installed", None)).is_none());
    }

    #[test]
    fn upgrades_and_copied_sign_ins_are_quiet_facts() {
        let mut r = row("connected", None);
        r.previous_version = Some("0.60.1".into());
        r.sign_in = Some("ChatGPT sign-in".into());
        r.signin_import = "ok".into();
        let v = connection_row(&r).unwrap();
        assert_eq!(v.detail.as_deref(), Some("0.61.0, updated from 0.60.1"));
        assert_eq!(v.sign_in.as_deref(), Some("ChatGPT sign-in in Keyvault"));
    }

    #[test]
    fn reads_the_status_json_as_the_cli_prints_it() {
        let j = serde_json::json!({"id":"hermes","name":"Hermes","installed":true,"version":"0.22.5",
            "previous_version":null,"signed_in":"no","sign_in":"provider key","mcp":"ok",
            "mcp_config":"~/.hermes/config.yaml","signin_imported":false,"signin_import":"missing",
            "state":"attention","problem":{"line":"Connected. No provider key on This Mac yet.","action":"none"},
            "checked_at_ms":1});
        let r: ConnectionInput = serde_json::from_value(j).unwrap();
        let v = connection_row(&r).unwrap();
        assert_eq!(v.label, "Connected. No provider key on This Mac yet.");
    }
}
