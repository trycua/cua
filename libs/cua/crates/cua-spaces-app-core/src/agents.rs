// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Coding agents running in a Space: a run record as a row, labels, the row
//! subtitle, search and the reading order; and the coding agents on this
//! machine (Settings and first run): each one's row and what a setup did.
//! The run status vocabulary is cua-agents' `RunStatus`, verbatim: a
//! status the SDK did not report is never invented.

use serde::{Deserialize, Serialize};

/// What an agent run is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentStatus {
    /// A turn is running.
    Running,
    /// Waiting for a follow-up.
    Idle,
    /// Failed.
    Failed,
    /// Crashed.
    Crashed,
    /// Could not be read.
    Unknown,
}

impl AgentStatus {
    /// The SDK's word (`running`, `idle`, ...); anything else is unknown.
    pub fn parse(word: &str) -> Self {
        match word.trim().to_ascii_lowercase().as_str() {
            "running" => AgentStatus::Running,
            "idle" => AgentStatus::Idle,
            "failed" => AgentStatus::Failed,
            "crashed" => AgentStatus::Crashed,
            _ => AgentStatus::Unknown,
        }
    }

    /// "Running", "Idle", ...
    pub fn label(self) -> &'static str {
        match self {
            AgentStatus::Running => "Running",
            AgentStatus::Idle => "Idle",
            AgentStatus::Failed => "Failed",
            AgentStatus::Crashed => "Crashed",
            AgentStatus::Unknown => "Unknown",
        }
    }

    fn attention_rank(self) -> u8 {
        match self {
            AgentStatus::Failed => 0,
            AgentStatus::Crashed => 1,
            AgentStatus::Idle => 2,
            AgentStatus::Running => 3,
            AgentStatus::Unknown => 4,
        }
    }
}

/// One agent run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceAgentRun {
    /// Run id.
    pub run_id: String,
    /// Harness id.
    pub agent: String,
    /// Status.
    pub status: AgentStatus,
    /// Why.
    pub reason: String,
    /// Prompt summary.
    pub summary: String,
    /// Unix seconds.
    pub created_at: Option<i64>,
    /// Finer phase.
    pub phase: String,
    /// Turns.
    pub turn: u32,
}

/// Longest summary line a row shows.
pub const SUMMARY_CHARS: usize = 120;

/// `text` as one line of at most [`SUMMARY_CHARS`] characters.
pub fn one_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= SUMMARY_CHARS {
        line
    } else {
        let cut: String = line.chars().take(SUMMARY_CHARS - 1).collect();
        format!("{cut}\u{2026}")
    }
}

/// One run record as the SDK reports it (cua-agents' `RunInfo` as JSON:
/// the `agent_list` tool's `runs[]`, `Space.agent_list()[].json`) as a row.
/// The harness comes from the record, else its `meta`; the summary is the
/// run's label, else its prompt, on one line.
pub fn agent_run_from_json(run: &serde_json::Value) -> SpaceAgentRun {
    let s = |v: &serde_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let meta = run.get("meta").filter(|m| m.is_object());
    let summary = meta
        .and_then(|m| {
            s(m, "label")
                .filter(|l| !l.trim().is_empty())
                .or_else(|| s(m, "prompt"))
        })
        .map(|t| one_line(&t))
        .unwrap_or_default();
    SpaceAgentRun {
        run_id: s(run, "run_id").unwrap_or_default(),
        agent: s(run, "harness")
            .or_else(|| s(run, "agent"))
            .or_else(|| meta.and_then(|m| s(m, "harness")))
            .unwrap_or_default(),
        status: AgentStatus::parse(&s(run, "status").unwrap_or_default()),
        reason: s(run, "reason").unwrap_or_default(),
        summary,
        created_at: meta
            .and_then(|m| m.get("created_at"))
            .and_then(|c| c.as_f64())
            .map(|c| c as i64),
        phase: s(run, "phase").unwrap_or_default(),
        turn: run
            .get("turn")
            .and_then(|t| t.as_u64())
            .map(|t| t.min(u32::MAX as u64) as u32)
            .unwrap_or(0),
    }
}

/// The Space detail's agent rows: every record, attention first, then newest.
pub fn agent_rows_from_json(runs: &[serde_json::Value]) -> Vec<SpaceAgentRun> {
    order_agent_runs(&runs.iter().map(agent_run_from_json).collect::<Vec<_>>())
}

/// Display name for a harness id.
pub fn agent_name(agent: &str) -> String {
    match agent {
        "" => "Unknown agent",
        "claude-code" => "Claude Code",
        "openai-codex" => "OpenAI Codex",
        "gemini-cli" => "Gemini CLI",
        "google-antigravity" => "Google Antigravity",
        "opencode" => "OpenCode",
        "goose" => "Goose",
        "pi" => "Pi",
        "hermes" => "Hermes",
        "openclaw" => "OpenClaw",
        other => other,
    }
    .to_string()
}

/// Monogram for a harness.
pub fn agent_initial(agent: &str) -> String {
    agent_name(agent)
        .trim()
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into())
}

/// The line under the agent's name.
pub fn agent_subtitle(run: &SpaceAgentRun) -> String {
    let s = run.summary.trim();
    if !s.is_empty() {
        s.to_string()
    } else if run.agent.is_empty() {
        "this run's record could not be read".into()
    } else {
        "no prompt recorded for this run".into()
    }
}

/// Runs matching the search box.
pub fn filter_agent_runs(runs: &[SpaceAgentRun], query: &str) -> Vec<SpaceAgentRun> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return runs.to_vec();
    }
    runs.iter()
        .filter(|r| {
            [
                r.run_id.as_str(),
                &agent_name(&r.agent),
                &r.summary,
                r.status.label(),
            ]
            .join(" ")
            .to_lowercase()
            .contains(&needle)
        })
        .cloned()
        .collect()
}

/// Whatever needs a human first, then newest.
pub fn order_agent_runs(runs: &[SpaceAgentRun]) -> Vec<SpaceAgentRun> {
    let mut out = runs.to_vec();
    out.sort_by(|a, b| {
        a.status
            .attention_rank()
            .cmp(&b.status.attention_rank())
            .then_with(|| b.created_at.unwrap_or(0).cmp(&a.created_at.unwrap_or(0)))
    });
    out
}

/// A coding agent as the SDK's agent onboarding (`cua-agent-setup`) sees
/// it on this machine: installed, and what cua configured for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentSetupStatus {
    /// Registry id (`claude-code`, `codex`, ...).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Looks installed.
    pub installed: bool,
    /// User skills directory, when it supports skills.
    pub skills_dir: Option<String>,
    /// MCP config file, when it supports MCP.
    pub mcp_config: Option<String>,
    /// The cua MCP server is configured.
    pub cua_configured: bool,
    /// Bundled skills present.
    pub skills_installed: Vec<String>,
    /// Installed skills that differ from the bundled copy.
    pub skills_outdated: Vec<String>,
    /// Problem reading its config.
    pub error: Option<String>,
}

/// One Settings row for a coding agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSettingsRow {
    /// Registry id.
    pub agent: String,
    /// Display name.
    pub name: String,
    /// Installed.
    pub installed: bool,
    /// cua skills and the cua MCP server are both in place (where supported).
    pub configured: bool,
    /// One line for the row.
    pub detail: String,
    /// Bundled skills present.
    pub skills_installed: u32,
    /// Bundled skills.
    pub skills_total: u32,
    /// MCP config file.
    pub mcp_config: Option<String>,
    /// Skills directory.
    pub skills_dir: Option<String>,
}

/// One agent's row (`total`: skills bundled with this SDK).
pub fn agent_settings_row(a: &AgentSetupStatus, total: u32) -> AgentSettingsRow {
    let skills_ok = a.skills_dir.is_none()
        || (a.skills_installed.len() as u32 >= total && a.skills_outdated.is_empty());
    let mcp_ok = a.mcp_config.is_none() || a.cua_configured;
    let configured = a.installed && skills_ok && mcp_ok;
    let detail = if let Some(e) = a.error.as_ref().filter(|e| !e.is_empty()) {
        e.clone()
    } else if !a.installed {
        "not installed".into()
    } else if configured {
        match (a.skills_dir.is_some(), a.mcp_config.is_some()) {
            (true, true) => "skills and MCP configured".into(),
            (true, false) => "skills installed (no MCP support)".into(),
            _ => "MCP configured".into(),
        }
    } else {
        let mut missing = Vec::new();
        if !skills_ok {
            missing.push(format!("skills {}/{total}", a.skills_installed.len()));
        }
        if !mcp_ok {
            missing.push("MCP not configured".to_string());
        }
        missing.join(", ")
    };
    AgentSettingsRow {
        agent: a.id.clone(),
        name: a.name.clone(),
        installed: a.installed,
        configured,
        detail,
        skills_installed: a.skills_installed.len() as u32,
        skills_total: total,
        mcp_config: a.mcp_config.clone(),
        skills_dir: a.skills_dir.clone(),
    }
}

/// Every agent's row, installed ones first (otherwise in registry order).
pub fn agent_settings_rows(statuses: &[AgentSetupStatus], total: u32) -> Vec<AgentSettingsRow> {
    let mut rows: Vec<AgentSettingsRow> = statuses
        .iter()
        .map(|a| agent_settings_row(a, total))
        .collect();
    rows.sort_by_key(|r| !r.installed);
    rows
}

/// One target's outcome of an agent setup step (the SDK's
/// `AgentSetupOutcome`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentSetupOutcomeInput {
    /// Agents it served.
    pub agents: Vec<String>,
    /// `skill` or `mcp`.
    pub target: String,
    /// Skill or server name.
    pub item: String,
    /// `created`, `updated`, `unchanged`, `removed`, `skipped`, `failed`.
    pub change: String,
    /// Explanation.
    pub detail: String,
}

/// What an agent setup did for one agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSetupSummary {
    /// "cua MCP server configured, 3 skills installed".
    pub text: String,
    /// "name: why" per failed target.
    pub failed: Vec<String>,
    /// "Claude Code: done" / "Claude Code: failed".
    pub line: String,
}

/// The background computer-use step's MCP server and skill
/// (`cua agents setup --cua-driver`).
pub const DRIVER_ITEM: &str = "cua-driver";

/// Summarises `outcomes` for `agent` (named `name`).
pub fn agent_setup_summary(
    outcomes: &[AgentSetupOutcomeInput],
    agent: &str,
    name: &str,
) -> AgentSetupSummary {
    let mine: Vec<&AgentSetupOutcomeInput> = outcomes
        .iter()
        .filter(|o| o.agents.iter().any(|a| a == agent))
        .collect();
    let failed: Vec<String> = mine
        .iter()
        .filter(|o| o.change == "failed")
        .map(|o| {
            format!(
                "{}: {}",
                o.item,
                if o.detail.is_empty() {
                    "failed"
                } else {
                    &o.detail
                }
            )
        })
        .collect();
    let ok: Vec<&&AgentSetupOutcomeInput> = mine
        .iter()
        .filter(|o| o.change != "failed" && o.change != "skipped")
        .collect();
    let skills = ok.iter().filter(|o| o.target == "skill").count();
    let mut parts = Vec::new();
    if ok
        .iter()
        .any(|o| o.target == "mcp" && o.item != DRIVER_ITEM)
    {
        parts.push("cua MCP server configured".to_string());
    }
    if ok
        .iter()
        .any(|o| o.target == "mcp" && o.item == DRIVER_ITEM)
    {
        parts.push("cua-driver configured".to_string());
    }
    if skills > 0 {
        parts.push(format!(
            "{skills} skill{} installed",
            if skills == 1 { "" } else { "s" }
        ));
    }
    let text = if !parts.is_empty() {
        parts.join(", ")
    } else if !failed.is_empty() {
        "failed".into()
    } else {
        "nothing to change".into()
    };
    AgentSetupSummary {
        line: format!(
            "{name}: {}",
            if failed.is_empty() { "done" } else { "failed" }
        ),
        text,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summaries_are_one_bounded_line() {
        assert_eq!(one_line("fix\n  the   build"), "fix the build");
        let s = one_line(&"x".repeat(400));
        assert_eq!(s.chars().count(), SUMMARY_CHARS);
        assert!(s.ends_with('\u{2026}'));
    }

    #[test]
    fn a_run_record_becomes_a_row() {
        let run = agent_run_from_json(&json!({
            "run_id": "run-1", "harness": null, "status": "idle", "phase": "waiting",
            "reason": "waiting for a follow-up", "turn": 2, "alive": true,
            "accepts_message": true,
            "meta": {"run_id": "run-1", "harness": "goose", "prompt": "fix\n the build",
                     "cwd": "/root", "created_at": 12.7}
        }));
        assert_eq!(run.agent, "goose");
        assert_eq!(run.status, AgentStatus::Idle);
        assert_eq!((run.phase.as_str(), run.turn), ("waiting", 2));
        assert_eq!(run.summary, "fix the build");
        assert_eq!(run.created_at, Some(12));
        // A record that could not be read is still a row, and unknown stays unknown.
        let bare = agent_run_from_json(&json!({"run_id": "run-2", "status": "exploded"}));
        assert_eq!(
            (bare.agent.as_str(), bare.status),
            ("", AgentStatus::Unknown)
        );
        assert_eq!(agent_subtitle(&bare), "this run's record could not be read");
        // A label wins over the prompt.
        let labelled = agent_run_from_json(&json!({"run_id": "r", "status": "running",
            "meta": {"harness": "pi", "prompt": "long prompt", "label": "Nightly"}}));
        assert_eq!(labelled.summary, "Nightly");
    }

    #[test]
    fn rows_put_attention_first() {
        let rows = agent_rows_from_json(&[
            json!({"run_id": "a", "status": "running", "meta": {"created_at": 5.0}}),
            json!({"run_id": "b", "status": "failed", "meta": {"created_at": 1.0}}),
            json!({"run_id": "c", "status": "running", "meta": {"created_at": 9.0}}),
        ]);
        let ids: Vec<_> = rows.iter().map(|r| r.run_id.as_str()).collect();
        assert_eq!(ids, ["b", "c", "a"]);
    }
}
