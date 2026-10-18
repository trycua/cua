// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Coding agents running inside a Space, for the pop-out list window's AGENTS
//! section.
//!
//! Runs are started and tracked by `cua_spaces::agents` (detached, tagged
//! cua-spacesd processes; the same records `cua daemon mcp`'s
//! `agent_start` writes). The record becomes a row in the app core
//! ([`cua_spaces_app_core::agents::agent_run_from_json`]), the same mapping
//! the SwiftUI app uses. The status vocabulary is the SDK's, verbatim: the
//! panel never invents a status the SDK did not report.

use cua_spaces::agents::RunInfo;

/// The webview's `SpaceAgentRun` (camelCase): the app core's row.
pub use cua_spaces_app_core::agents::SpaceAgentRun;

/// One run record as a row.
pub fn space_agent_run(run: &RunInfo) -> SpaceAgentRun {
    let value = serde_json::to_value(run).unwrap_or_default();
    cua_spaces_app_core::agents::agent_run_from_json(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_spaces::agents::RunStatus;
    use cua_spaces_app_core::agents::AgentStatus;

    #[test]
    fn run_info_maps_to_the_webview_shape() {
        let info = RunInfo {
            run_id: "run-1".into(),
            harness: None,
            status: RunStatus::Idle,
            phase: "waiting".into(),
            reason: "waiting for a follow-up".into(),
            turn: 2,
            session_id: None,
            stop_reason: None,
            error: None,
            alive: Some(true),
            accepts_message: true,
            meta: Some(cua_spaces::agents::runs::RunMeta {
                run_id: "run-1".into(),
                harness: "goose".into(),
                prompt: "fix\n the build".into(),
                cwd: "/root".into(),
                created_at: 12.7,
                label: None,
                repo: None,
                home: None,
            }),
        };
        let run = space_agent_run(&info);
        assert_eq!(run.agent, "goose");
        assert_eq!(run.status, AgentStatus::Idle);
        assert_eq!(run.phase, "waiting");
        assert_eq!(run.turn, 2);
        assert_eq!(run.summary, "fix the build");
        assert_eq!(run.created_at, Some(12));
        // The webview reads the same camelCase JSON as before.
        let json = serde_json::to_value(&run).unwrap();
        assert_eq!(json["status"], "idle");
        assert_eq!(json["runId"], "run-1");
        assert_eq!(json["createdAt"], 12);
    }
}
