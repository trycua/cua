// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! First-run graphical installer steps (plan §8.14): put the bundled `cua`
//! CLI on PATH, then agent onboarding (skills + the cua MCP server in each
//! detected AI coding agent).
//!
//! The `cua` CLI ships inside every bundle as a Tauri sidecar (next to the
//! app executable). [`CliInstaller`] (the app core's, shared with the
//! SwiftUI app) shows the exact target path before anything is written and
//! only writes after the user consents in the UI.
//! Agent onboarding goes through [`AgentSetupBackend`]; the app uses
//! [`SdkAgentSetup`] (the SDK's `cua-agent-setup` in-process), the same
//! engine as `cua agents` and `cua auth login` (install.sh), so all paths
//! behave the same. [`CliAgentSetup`] (`cua agents … --json`) remains for
//! hosts without the crate.
//!
//! Everything here takes its paths and environment explicitly so tests run
//! against temp dirs and fake binaries only.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// The CLI step is the app core's, shared with the SwiftUI app.
pub use cua_spaces_app_core::installer::*;

const AGENTS_TIMEOUT: Duration = Duration::from_secs(120);

// ------------------------------------------------------------ agent onboarding

/// One AI coding agent as the onboarding screen shows it. Field names follow
/// `cua_agent_setup::AgentStatus` (snake_case in the CLI's `--json`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub installed: bool,
    #[serde(default, alias = "skills_dir")]
    pub skills_dir: Option<String>,
    #[serde(default, alias = "mcp_config")]
    pub mcp_config: Option<String>,
    /// The cua MCP server is already configured.
    #[serde(default, alias = "cua_configured")]
    pub cua_configured: bool,
    #[serde(default, alias = "skills_installed")]
    pub skills_installed: Vec<String>,
    #[serde(default, alias = "skills_outdated")]
    pub skills_outdated: Vec<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// A bundled cua skill.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInfo {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
}

/// `detect` result: agents plus the skills a setup would install.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDetectReport {
    pub agents: Vec<AgentInfo>,
    #[serde(default)]
    pub skills: Vec<SkillInfo>,
}

/// What the user ticked on the onboarding screen.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSetupRequest {
    /// Agent ids (registry ids, e.g. `claude-code`, `codex`).
    pub agents: Vec<String>,
    /// Install the default cua skills.
    pub skills: bool,
    /// Configure the cua MCP server.
    pub mcp: bool,
    /// Also set up background computer-use: the cua-driver skill and the
    /// cua-driver MCP server (`cua agents setup --cua-driver`).
    #[serde(default)]
    pub driver: bool,
}

/// One target's outcome (`cua_agent_setup::Outcome`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSetupOutcome {
    #[serde(default)]
    pub agents: Vec<String>,
    /// `skill` or `mcp`.
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub item: String,
    #[serde(default)]
    pub path: String,
    /// created / updated / unchanged / removed / skipped / failed.
    #[serde(default)]
    pub change: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub backup: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSetupReport {
    pub outcomes: Vec<AgentSetupOutcome>,
}

impl AgentSetupReport {
    pub fn failed(&self) -> usize {
        self.outcomes
            .iter()
            .filter(|o| o.change == "failed")
            .count()
    }
}

/// Agent onboarding, as the installer UI needs it.
#[async_trait]
pub trait AgentSetupBackend: Send + Sync {
    async fn detect(&self) -> Result<AgentDetectReport, String>;
    async fn setup(&self, request: AgentSetupRequest) -> Result<AgentSetupReport, String>;
}

/// Runs the bundled `cua agents detect|setup --json` (cua-agent-setup).
pub struct CliAgentSetup {
    cli: PathBuf,
    /// Extra environment (tests point HOME at a temp dir).
    env: Vec<(String, String)>,
}

impl CliAgentSetup {
    pub fn new(cli: PathBuf) -> Self {
        Self {
            cli,
            env: Vec::new(),
        }
    }

    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// The guard rails every backend applies to `request`: at least one
    /// agent, at least one thing to set up, and plain agent ids.
    pub fn validate(request: &AgentSetupRequest) -> Result<(), String> {
        if request.agents.is_empty() {
            return Err("pick at least one agent".into());
        }
        if !request.skills && !request.mcp && !request.driver {
            return Err("choose skills, the MCP server, or cua-driver".into());
        }
        if let Some(bad) = request.agents.iter().find(|a| {
            !a.starts_with(|c: char| c.is_ascii_alphanumeric())
                || !a
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        }) {
            return Err(format!("invalid agent id {bad:?}"));
        }
        Ok(())
    }

    /// The `cua agents setup` arguments for `request`'s skills and MCP
    /// server (an error when it asks for neither).
    pub fn setup_args(request: &AgentSetupRequest) -> Result<Vec<String>, String> {
        Self::validate(request)?;
        if !request.skills && !request.mcp {
            return Err("choose skills, the MCP server, or both".into());
        }
        let mut args = vec![
            "agents".to_string(),
            "setup".to_string(),
            "--agents".to_string(),
            request.agents.join(","),
        ];
        if !request.skills {
            args.push("--no-skills".into());
        }
        if !request.mcp {
            args.push("--no-mcp".into());
        }
        args.extend(["--yes".to_string(), "--json".to_string()]);
        Ok(args)
    }

    /// The `cua agents setup --cua-driver` arguments for `request` (the
    /// cua-driver skill and MCP server for the same agents).
    pub fn driver_args(request: &AgentSetupRequest) -> Result<Vec<String>, String> {
        Self::validate(request)?;
        Ok(vec![
            "agents".to_string(),
            "setup".to_string(),
            "--cua-driver".to_string(),
            "--agents".to_string(),
            request.agents.join(","),
            "--yes".to_string(),
            "--json".to_string(),
        ])
    }

    async fn run(&self, args: &[String]) -> Result<Vec<u8>, String> {
        let mut cmd = tokio::process::Command::new(&self.cli);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        let out = tokio::time::timeout(AGENTS_TIMEOUT, cmd.output())
            .await
            .map_err(|_| format!("`cua {}` timed out", args.join(" ")))?
            .map_err(|e| format!("cannot run {}: {e}", self.cli.display()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let err = err.trim();
            return Err(if err.is_empty() {
                format!("`cua {}` failed ({})", args.join(" "), out.status)
            } else {
                err.lines().last().unwrap_or(err).to_string()
            });
        }
        Ok(out.stdout)
    }
}

/// Accepts `[agents…]`, `{"agents":[…],"skills":[…]}` or the same wrapped as
/// `{"data":…}`.
pub fn parse_detect(bytes: &[u8]) -> Result<AgentDetectReport, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("invalid `cua agents detect --json` output: {e}"))?;
    let value = value.get("data").cloned().unwrap_or(value);
    let report = if value.is_array() {
        AgentDetectReport {
            agents: serde_json::from_value(value)
                .map_err(|e| format!("invalid agent list: {e}"))?,
            skills: Vec::new(),
        }
    } else {
        serde_json::from_value(value).map_err(|e| format!("invalid agent report: {e}"))?
    };
    Ok(report)
}

/// Accepts `[outcomes…]` or `{"outcomes":[…]}` (optionally under `data`).
pub fn parse_setup(bytes: &[u8]) -> Result<AgentSetupReport, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("invalid `cua agents setup --json` output: {e}"))?;
    let value = value.get("data").cloned().unwrap_or(value);
    let outcomes = if value.is_array() {
        value
    } else {
        value.get("outcomes").cloned().unwrap_or_default()
    };
    let mut outcomes: Vec<AgentSetupOutcome> =
        serde_json::from_value(outcomes).map_err(|e| format!("invalid setup outcomes: {e}"))?;
    for o in &mut outcomes {
        o.change = o.change.to_ascii_lowercase();
    }
    Ok(AgentSetupReport { outcomes })
}

#[async_trait]
impl AgentSetupBackend for CliAgentSetup {
    async fn detect(&self) -> Result<AgentDetectReport, String> {
        let out = self
            .run(&["agents".into(), "detect".into(), "--json".into()])
            .await?;
        parse_detect(&out)
    }

    async fn setup(&self, request: AgentSetupRequest) -> Result<AgentSetupReport, String> {
        Self::validate(&request)?;
        let mut report = AgentSetupReport::default();
        if request.skills || request.mcp {
            let out = self.run(&Self::setup_args(&request)?).await?;
            report.outcomes.extend(parse_setup(&out)?.outcomes);
        }
        if request.driver {
            let out = self.run(&Self::driver_args(&request)?).await?;
            report.outcomes.extend(parse_setup(&out)?.outcomes);
        }
        Ok(report)
    }
}

/// Agent onboarding in-process through the SDK's `cua-agent-setup` (the
/// engine behind `cua agents` and `cua auth login`), so the installer does
/// not depend on a CLI being present. The MCP entry launches `mcp_command`
/// (the installed `cua`, or the bundled one).
pub struct SdkAgentSetup {
    setup: cua_agent_setup::AgentSetup,
    mcp_command: Option<PathBuf>,
}

impl SdkAgentSetup {
    pub fn new(setup: cua_agent_setup::AgentSetup, mcp_command: Option<PathBuf>) -> Self {
        Self { setup, mcp_command }
    }

    fn server(&self) -> cua_agent_setup::McpServer {
        match &self.mcp_command {
            Some(p) => cua_agent_setup::McpServer::new(p.display().to_string()),
            None => cua_agent_setup::McpServer::default_for(self.setup.env()),
        }
    }
}

fn outcome(o: cua_agent_setup::Outcome) -> AgentSetupOutcome {
    // Same snake_case words as the CLI's `--json`.
    let v = serde_json::to_value(&o).unwrap_or_default();
    AgentSetupOutcome {
        agents: o.agents,
        target: v["target"].as_str().unwrap_or_default().to_string(),
        item: o.item,
        path: o.path.display().to_string(),
        change: v["change"].as_str().unwrap_or_default().to_string(),
        detail: o.detail,
        backup: o.backup.map(|b| b.display().to_string()),
    }
}

#[async_trait]
impl AgentSetupBackend for SdkAgentSetup {
    async fn detect(&self) -> Result<AgentDetectReport, String> {
        let setup = self.setup.clone();
        tokio::task::spawn_blocking(move || AgentDetectReport {
            agents: setup
                .detect()
                .into_iter()
                .map(|a| AgentInfo {
                    id: a.id,
                    name: a.name,
                    installed: a.installed,
                    skills_dir: a.skills_dir.map(|p| p.display().to_string()),
                    mcp_config: a.mcp_config.map(|p| p.display().to_string()),
                    cua_configured: a.cua_configured,
                    skills_installed: a.skills_installed,
                    skills_outdated: a.skills_outdated,
                    error: a.error,
                })
                .collect(),
            skills: setup
                .bundled_skills()
                .into_iter()
                .map(|s| SkillInfo {
                    name: s.name,
                    description: s.description,
                    version: s.version,
                })
                .collect(),
        })
        .await
        .map_err(|e| e.to_string())
    }

    async fn setup(&self, request: AgentSetupRequest) -> Result<AgentSetupReport, String> {
        // Same validation as the CLI path.
        CliAgentSetup::validate(&request)?;
        let setup = self.setup.clone();
        let server = self.server();
        tokio::task::spawn_blocking(move || {
            let mut outcomes = Vec::new();
            if request.skills {
                outcomes.extend(
                    setup
                        .install_skills(&request.agents, &[], false)
                        .map_err(|e| e.to_string())?,
                );
            }
            if request.mcp {
                outcomes.extend(
                    setup
                        .configure_mcp(&request.agents, &server)
                        .map_err(|e| e.to_string())?,
                );
            }
            if request.driver {
                let driver = cua_agent_setup::McpServer::driver_for(setup.env());
                outcomes.extend(
                    setup
                        .setup_cua_driver(
                            &request.agents,
                            &driver,
                            cua_agent_setup::Parts::ALL,
                            false,
                        )
                        .map_err(|e| e.to_string())?,
                );
            }
            Ok(AgentSetupReport {
                outcomes: outcomes.into_iter().map(outcome).collect(),
            })
        })
        .await
        .map_err(|e| e.to_string())?
    }
}

/// Used when the build has no bundled CLI (dev builds without the sidecar).
pub struct UnavailableAgentSetup;

#[async_trait]
impl AgentSetupBackend for UnavailableAgentSetup {
    async fn detect(&self) -> Result<AgentDetectReport, String> {
        Err("agent setup needs the cua CLI; install it first or run `cua agents setup`".into())
    }
    async fn setup(&self, _request: AgentSetupRequest) -> Result<AgentSetupReport, String> {
        self.detect().await.map(|_| AgentSetupReport::default())
    }
}

/// Everything the `installer_*` commands need.
pub struct InstallerCommands {
    pub cli: CliInstaller,
    pub agents: Arc<dyn AgentSetupBackend>,
}

impl InstallerCommands {
    pub fn new(cli: CliInstaller, agents: Arc<dyn AgentSetupBackend>) -> Self {
        Self { cli, agents }
    }

    /// The real machine: agent setup in-process through the SDK; the MCP
    /// entry launches the installed CLI when present, else the bundled one.
    pub fn from_env(exe: &Path) -> Self {
        let cli = CliInstaller::from_env(exe);
        let target = cli.target();
        let mcp_command = if target.exists() {
            Some(target)
        } else {
            cli.bundled
                .clone()
                .or_else(|| first_on_path(&cli.path_env, cli_file_name()))
        };
        let agents: Arc<dyn AgentSetupBackend> = Arc::new(SdkAgentSetup::new(
            cua_agent_setup::AgentSetup::from_env(),
            mcp_command,
        ));
        Self::new(cli, agents)
    }

    pub async fn cli_plan(&self) -> CliInstallPlan {
        self.cli.plan().await
    }

    pub async fn install_cli(&self, request: CliInstallRequest) -> Result<CliInstallPlan, String> {
        self.cli.install(&request).await
    }

    pub async fn detect_agents(&self) -> Result<AgentDetectReport, String> {
        self.agents.detect().await
    }

    pub async fn setup_agents(
        &self,
        request: AgentSetupRequest,
    ) -> Result<AgentSetupReport, String> {
        // Validate here too so every backend gets the same guard rails.
        CliAgentSetup::validate(&request)?;
        self.agents.setup(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sdk_backend_detects_and_sets_up_in_a_temp_home() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".codex")).unwrap();
        let b = SdkAgentSetup::new(
            cua_agent_setup::AgentSetup::new(cua_agent_setup::HostEnv::isolated(d.path())),
            Some(PathBuf::from("/usr/local/bin/cua")),
        );
        let r = b.detect().await.unwrap();
        let codex = r.agents.iter().find(|a| a.id == "codex").unwrap();
        assert!(codex.installed && !codex.cua_configured);
        assert!(r.skills.iter().any(|s| s.name == "cua-spaces"));
        let rep = b
            .setup(AgentSetupRequest {
                agents: vec!["codex".into()],
                skills: true,
                mcp: true,
                driver: false,
            })
            .await
            .unwrap();
        assert_eq!(rep.failed(), 0, "{rep:?}");
        assert!(rep
            .outcomes
            .iter()
            .any(|o| o.target == "mcp" && o.change == "created"));
        let toml = std::fs::read_to_string(d.path().join(".codex/config.toml")).unwrap();
        assert!(toml.contains("command = \"/usr/local/bin/cua\""), "{toml}");
        assert!(b
            .setup(AgentSetupRequest {
                agents: vec![],
                skills: true,
                mcp: true,
                driver: false,
            })
            .await
            .is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sdk_backend_sets_up_cua_driver_alone_or_with_skills_and_mcp() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".codex")).unwrap();
        // cua-driver's installer puts it in ~/.local/bin.
        let bin = d.path().join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let driver = bin.join("cua-driver");
        std::fs::write(&driver, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&driver, std::fs::Permissions::from_mode(0o755)).unwrap();
        let b = SdkAgentSetup::new(
            cua_agent_setup::AgentSetup::new(cua_agent_setup::HostEnv::isolated(d.path())),
            Some(PathBuf::from("/usr/local/bin/cua")),
        );
        let only = b
            .setup(AgentSetupRequest {
                agents: vec!["codex".into()],
                skills: false,
                mcp: false,
                driver: true,
            })
            .await
            .unwrap();
        assert_eq!(only.failed(), 0, "{only:?}");
        let items: Vec<_> = only
            .outcomes
            .iter()
            .map(|o| (o.target.as_str(), o.item.as_str(), o.change.as_str()))
            .collect();
        assert_eq!(
            items,
            [
                ("skill", "cua-driver", "created"),
                ("mcp", "cua-driver", "created")
            ]
        );
        let toml = std::fs::read_to_string(d.path().join(".codex/config.toml")).unwrap();
        assert!(
            toml.contains(&format!("command = \"{}\"", driver.display())),
            "{toml}"
        );
        assert!(!toml.contains("/usr/local/bin/cua\""), "{toml}");
        // With skills and MCP too: both steps' outcomes, the cua entry first.
        let all = b
            .setup(AgentSetupRequest {
                agents: vec!["codex".into()],
                skills: true,
                mcp: true,
                driver: true,
            })
            .await
            .unwrap();
        assert_eq!(all.failed(), 0, "{all:?}");
        assert!(all
            .outcomes
            .iter()
            .any(|o| o.target == "mcp" && o.item == "cua" && o.change == "created"));
        assert_eq!(
            all.outcomes
                .last()
                .map(|o| (o.item.as_str(), o.change.as_str())),
            Some(("cua-driver", "unchanged"))
        );
    }

    #[test]
    fn driver_args_follow_the_cli_contract() {
        let request = AgentSetupRequest {
            agents: vec!["claude-code".into(), "codex".into()],
            skills: false,
            mcp: false,
            driver: true,
        };
        assert_eq!(
            CliAgentSetup::driver_args(&request).unwrap(),
            [
                "agents",
                "setup",
                "--cua-driver",
                "--agents",
                "claude-code,codex",
                "--yes",
                "--json"
            ]
        );
        // Driver alone is a valid request, but has no skills/MCP args.
        assert!(CliAgentSetup::validate(&request).is_ok());
        assert!(CliAgentSetup::setup_args(&request).is_err());
        assert!(CliAgentSetup::driver_args(&AgentSetupRequest {
            agents: vec!["--all".into()],
            ..request
        })
        .is_err());
    }

    #[test]
    fn setup_args_follow_the_cli_contract() {
        let args = CliAgentSetup::setup_args(&AgentSetupRequest {
            agents: vec!["claude-code".into(), "codex".into()],
            skills: true,
            mcp: false,
            driver: false,
        })
        .unwrap();
        assert_eq!(
            args,
            [
                "agents",
                "setup",
                "--agents",
                "claude-code,codex",
                "--no-mcp",
                "--yes",
                "--json"
            ]
        );
    }

    #[test]
    fn setup_args_reject_empty_and_odd_input() {
        let base = AgentSetupRequest {
            agents: vec!["codex".into()],
            skills: true,
            mcp: true,
            driver: false,
        };
        assert!(CliAgentSetup::setup_args(&AgentSetupRequest {
            agents: vec![],
            ..base.clone()
        })
        .is_err());
        let nothing = AgentSetupRequest {
            skills: false,
            mcp: false,
            ..base.clone()
        };
        assert!(CliAgentSetup::setup_args(&nothing).is_err());
        assert_eq!(
            CliAgentSetup::validate(&nothing).unwrap_err(),
            "choose skills, the MCP server, or cua-driver"
        );
        assert!(CliAgentSetup::setup_args(&AgentSetupRequest {
            agents: vec!["a,b".into()],
            ..base.clone()
        })
        .is_err());
        assert!(CliAgentSetup::setup_args(&AgentSetupRequest {
            agents: vec!["--all".into()],
            ..base
        })
        .is_err());
    }

    #[test]
    fn parses_snake_case_status_list_and_wrapped_reports() {
        let list = br#"[{"id":"codex","name":"Codex","installed":true,"skills_dir":"/h/.codex/skills",
            "mcp_config":"/h/.codex/config.toml","cua_configured":false,"skills_installed":["cua-driver"],
            "evidence":["dir:~/.codex"]}]"#;
        let r = parse_detect(list).unwrap();
        assert_eq!(r.agents[0].skills_dir.as_deref(), Some("/h/.codex/skills"));
        assert_eq!(r.agents[0].skills_installed, ["cua-driver"]);
        let wrapped = br#"{"data":{"agents":[{"id":"cursor","name":"Cursor"}],"skills":[{"name":"cua-driver","description":"d","version":"1"}]}}"#;
        let r = parse_detect(wrapped).unwrap();
        assert!(!r.agents[0].installed);
        assert_eq!(r.skills[0].name, "cua-driver");
        assert!(parse_detect(b"not json").is_err());
    }

    #[test]
    fn parses_setup_outcomes() {
        let r = parse_setup(br#"{"outcomes":[{"agents":["codex"],"target":"mcp","item":"cua","path":"/p","change":"Created","detail":"","backup":null},
            {"agents":["cursor"],"target":"skill","item":"cua-driver","path":"/q","change":"failed","detail":"denied"}]}"#).unwrap();
        assert_eq!(r.outcomes[0].change, "created");
        assert_eq!(r.failed(), 1);
        assert_eq!(parse_setup(b"[]").unwrap().outcomes.len(), 0);
    }
}
