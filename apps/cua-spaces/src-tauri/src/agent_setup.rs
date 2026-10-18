// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Settings "AI agents": the SDK's agent onboarding (`cua-agent-setup`, the
//! same engine as `cua agents setup` and `cua auth login`). Detects installed
//! coding agents, installs the bundled cua skills and registers the cua MCP
//! server (`cua mcp`) in each agent's own config with a structured,
//! backed-up, reversible edit. The app keeps no config writers of its own.

use cua_agent_setup::{AgentSetup, AgentStatus, Change, McpServer, Outcome, Parts};

/// One settings row: the app core's (`agents::agent_settings_row`), the
/// same rows the SwiftUI app shows.
pub use cua_spaces_app_core::agents::AgentSettingsRow as AgentRow;

fn status(a: &AgentStatus) -> cua_spaces_app_core::agents::AgentSetupStatus {
    cua_spaces_app_core::agents::AgentSetupStatus {
        id: a.id.clone(),
        name: a.name.clone(),
        installed: a.installed,
        skills_dir: a.skills_dir.as_ref().map(|p| p.display().to_string()),
        mcp_config: a.mcp_config.as_ref().map(|p| p.display().to_string()),
        cua_configured: a.cua_configured,
        skills_installed: a.skills_installed.clone(),
        skills_outdated: a.skills_outdated.clone(),
        error: a.error.clone(),
    }
}

/// Every supported agent, installed ones first.
pub fn detect(setup: &AgentSetup) -> Vec<AgentRow> {
    let total = setup.bundled_skills().len() as u32;
    let statuses: Vec<_> = setup.detect().iter().map(status).collect();
    cua_spaces_app_core::agents::agent_settings_rows(&statuses, total)
}

/// Installs the skills and configures `server` for `agents` (`None`: every
/// installed agent). Returns the fresh rows of those agents; a failed step
/// shows in the row's detail.
pub fn configure(
    setup: &AgentSetup,
    agents: Option<Vec<String>>,
    server: &McpServer,
) -> Result<Vec<AgentRow>, String> {
    let ids = match agents {
        Some(a) => a,
        None => setup.select("all").map_err(|e| e.to_string())?,
    };
    let mut failures: Vec<Outcome> = setup
        .install_skills(&ids, &[], false)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|o| o.change == Change::Failed)
        .collect();
    failures.extend(
        setup
            .configure_mcp(&ids, server)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|o| o.change == Change::Failed),
    );
    Ok(rows_for(setup, &ids, &failures))
}

/// Removes what cua added for `agents`.
pub fn remove(setup: &AgentSetup, agents: Vec<String>) -> Result<Vec<AgentRow>, String> {
    let failures: Vec<Outcome> = setup
        .remove(&agents, Parts::ALL)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|o| o.change == Change::Failed)
        .collect();
    Ok(rows_for(setup, &agents, &failures))
}

fn rows_for(setup: &AgentSetup, ids: &[String], failures: &[Outcome]) -> Vec<AgentRow> {
    detect(setup)
        .into_iter()
        .filter(|r| ids.contains(&r.agent))
        .map(|mut r| {
            let errs: Vec<&str> = failures
                .iter()
                .filter(|o| o.agents.contains(&r.agent))
                .map(|o| o.detail.as_str())
                .collect();
            if !errs.is_empty() {
                r.configured = false;
                r.detail = errs.join("; ");
            }
            r
        })
        .collect()
}

/// `cua mcp`, launched through the app's own `cua` (sidecar, `CUA_BIN` or
/// PATH) so GUI agents do not depend on a shell PATH.
fn server(setup: &AgentSetup) -> McpServer {
    match crate::core::find_cua_bin() {
        Some(p) => McpServer::new(p.display().to_string()),
        None => McpServer::default_for(setup.env()),
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce(AgentSetup) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || f(AgentSetup::from_env()))
        .await
        .map_err(|e| e.to_string())?
}

/// Detect every agent (read-only).
#[tauri::command]
pub async fn agent_setup_detect() -> Result<Vec<AgentRow>, String> {
    blocking(|s| Ok(detect(&s))).await
}

/// Set up `agents` (all installed when omitted).
#[tauri::command]
pub async fn agent_setup_configure(agents: Option<Vec<String>>) -> Result<Vec<AgentRow>, String> {
    blocking(move |s| {
        let srv = server(&s);
        configure(&s, agents, &srv)
    })
    .await
}

/// Undo what cua added for `agents`.
#[tauri::command]
pub async fn agent_setup_remove(agents: Vec<String>) -> Result<Vec<AgentRow>, String> {
    blocking(move |s| remove(&s, agents)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_agent_setup::HostEnv;

    fn home() -> (tempfile::TempDir, AgentSetup) {
        let d = tempfile::tempdir().unwrap();
        let s = AgentSetup::new(HostEnv::isolated(d.path()));
        (d, s)
    }

    #[test]
    fn rows_reflect_detection_and_list_installed_first() {
        let (d, s) = home();
        std::fs::create_dir_all(d.path().join(".codex")).unwrap();
        std::fs::create_dir_all(d.path().join(".pi/agent")).unwrap();
        let rows = detect(&s);
        assert_eq!(rows[0].agent, "codex");
        assert_eq!(rows[1].agent, "pi");
        assert!(rows[..2].iter().all(|r| r.installed && !r.configured));
        assert!(rows[2..]
            .iter()
            .all(|r| !r.installed && r.detail == "not installed"));
        assert!(
            rows[0].detail.contains("MCP not configured"),
            "{}",
            rows[0].detail
        );
    }

    #[test]
    fn configure_all_installed_then_remove_one() {
        let (d, s) = home();
        std::fs::create_dir_all(d.path().join(".codex")).unwrap();
        std::fs::create_dir_all(d.path().join(".cursor")).unwrap();
        std::fs::create_dir_all(d.path().join(".pi/agent")).unwrap();
        // An unrelated server the user already has must survive.
        std::fs::write(
            d.path().join(".cursor/mcp.json"),
            "{\n  // mine\n  \"mcpServers\": {\"other\": {\"command\": \"o\"}}\n}\n",
        )
        .unwrap();
        let srv = McpServer::new("/Applications/Cua Spaces.app/Contents/MacOS/cua");
        let rows = configure(&s, None, &srv).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows.iter().all(|r| r.configured), "{rows:#?}");
        let pi = rows.iter().find(|r| r.agent == "pi").unwrap();
        assert_eq!(pi.detail, "skills installed (no MCP support)");
        let cursor = std::fs::read_to_string(d.path().join(".cursor/mcp.json")).unwrap();
        assert!(
            cursor.contains("// mine") && cursor.contains("\"other\""),
            "{cursor}"
        );
        assert!(
            cursor.contains("Cua Spaces.app/Contents/MacOS/cua"),
            "{cursor}"
        );
        let codex = std::fs::read_to_string(d.path().join(".codex/config.toml")).unwrap();
        assert!(codex.contains("args = [\"mcp\"]"), "{codex}");

        let rows = remove(&s, vec!["cursor".into()]).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].configured);
        let cursor = std::fs::read_to_string(d.path().join(".cursor/mcp.json")).unwrap();
        assert!(
            cursor.contains("\"other\"") && !cursor.contains("Cua Spaces"),
            "{cursor}"
        );
        // Shared skills stay for codex and pi.
        assert!(d
            .path()
            .join(".agents/skills/cua-spaces/SKILL.md")
            .is_file());
    }

    #[test]
    fn a_malformed_config_shows_in_the_row_and_is_not_touched() {
        let (d, s) = home();
        std::fs::create_dir_all(d.path().join(".kiro/settings")).unwrap();
        std::fs::write(d.path().join(".kiro/settings/mcp.json"), "{ nope").unwrap();
        let rows = configure(&s, Some(vec!["kiro".into()]), &McpServer::new("cua")).unwrap();
        assert!(!rows[0].configured);
        assert!(
            rows[0].detail.contains("left unchanged"),
            "{}",
            rows[0].detail
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join(".kiro/settings/mcp.json")).unwrap(),
            "{ nope"
        );
    }

    #[test]
    fn unknown_agents_are_an_error() {
        let (_d, s) = home();
        assert!(
            configure(&s, Some(vec!["nope".into()]), &McpServer::new("cua"))
                .unwrap_err()
                .contains("unknown agent")
        );
    }
}
