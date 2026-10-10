// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The first-run installer command layer (`cua_spaces_lib::installer`) that
//! the `installer_*` Tauri commands call. Hermetic: temp dirs, a fake `cua`
//! script as the sidecar, an injected PATH. Never touches the real home, PATH
//! or shell profiles.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cua_spaces_lib::installer::{
    AgentDetectReport, AgentInfo, AgentSetupBackend, AgentSetupReport, AgentSetupRequest,
    CliAgentSetup, CliInstallRequest, CliInstaller, InstallMethod, InstallerCommands,
};

fn installer(dir: &Path, bundled: Option<PathBuf>, path_env: &str, symlink: bool) -> CliInstaller {
    CliInstaller {
        bundled,
        bin_dir: dir.join("home/.local/bin"),
        path_env: path_env.to_string(),
        profile: dir.join("home/.zshrc"),
        prefer_symlink: symlink,
    }
}

#[cfg(unix)]
fn fake_cua(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let app = dir.join("Cua Spaces.app/Contents/MacOS");
    std::fs::create_dir_all(&app).unwrap();
    let path = app.join("cua");
    // Write a staging copy and let a child process create the executable:
    // a file this (multithreaded) process opened for writing can be held
    // open by a sibling test's fork until its exec, so exec'ing it races
    // into ETXTBSY ("Text file busy"). The child's fds never leak here.
    let staging = dir.join("cua.src");
    std::fs::write(&staging, format!("#!/bin/sh\n{body}\n")).unwrap();
    let status = std::process::Command::new("cp")
        .arg(&staging)
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success(), "cp {staging:?} {path:?}");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[cfg(unix)]
const VERSION: &str = r#"if [ "$1" = --version ]; then echo "cua 9.9.9"; exit 0; fi"#;

#[cfg(unix)]
#[tokio::test]
async fn plan_shows_the_exact_target_and_path_hint_before_installing() {
    let tmp = tempfile::tempdir().unwrap();
    let cua = fake_cua(tmp.path(), VERSION);
    let cli = installer(tmp.path(), Some(cua.clone()), "/usr/bin:/bin", false);
    let plan = cli.plan().await;
    let target = tmp.path().join("home/.local/bin/cua");
    assert_eq!(plan.target, target.display().to_string());
    assert_eq!(plan.source.as_deref(), Some(cua.to_str().unwrap()));
    assert!(!plan.installed && !plan.on_path && !plan.up_to_date);
    assert_eq!(plan.method, Some(InstallMethod::Copy));
    assert_eq!(plan.bundled_version.as_deref(), Some("cua 9.9.9"));
    assert_eq!(
        plan.path_profile.as_deref(),
        Some(tmp.path().join("home/.zshrc").to_str().unwrap())
    );
    assert!(plan.path_line.unwrap().contains(".local/bin"));
    // Planning wrote nothing.
    assert!(!tmp.path().join("home").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn install_copies_the_sidecar_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let cua = fake_cua(tmp.path(), VERSION);
    let cli = installer(tmp.path(), Some(cua), "/usr/bin", false);
    let plan = cli.install(&CliInstallRequest::default()).await.unwrap();
    assert!(plan.installed && plan.up_to_date);
    assert_eq!(plan.installed_version.as_deref(), Some("cua 9.9.9"));
    let target = tmp.path().join("home/.local/bin/cua");
    assert!(!target.symlink_metadata().unwrap().file_type().is_symlink());
    // Without consent to modify PATH the profile is untouched.
    assert!(!tmp.path().join("home/.zshrc").exists());
    // Reinstall replaces in place; no staging file left behind.
    cli.install(&CliInstallRequest::default()).await.unwrap();
    let names: Vec<_> = std::fs::read_dir(tmp.path().join("home/.local/bin"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, ["cua"]);
}

#[cfg(unix)]
#[tokio::test]
async fn install_symlinks_into_a_stable_app_bundle() {
    let tmp = tempfile::tempdir().unwrap();
    let cua = fake_cua(tmp.path(), VERSION);
    let cli = installer(tmp.path(), Some(cua.clone()), "/usr/bin", true);
    let plan = cli.install(&CliInstallRequest::default()).await.unwrap();
    assert_eq!(plan.method, Some(InstallMethod::Symlink));
    let target = tmp.path().join("home/.local/bin/cua");
    assert_eq!(std::fs::read_link(&target).unwrap(), cua);
    assert!(plan.up_to_date);
}

#[cfg(unix)]
#[tokio::test]
async fn modify_path_appends_once_to_the_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let cua = fake_cua(tmp.path(), VERSION);
    std::fs::create_dir_all(tmp.path().join("home")).unwrap();
    std::fs::write(tmp.path().join("home/.zshrc"), "alias ll='ls -l'").unwrap();
    let cli = installer(tmp.path(), Some(cua), "/usr/bin", false);
    let request = CliInstallRequest { modify_path: true };
    let plan = cli.install(&request).await.unwrap();
    assert!(plan.on_path);
    cli.install(&request).await.unwrap();
    let profile = std::fs::read_to_string(tmp.path().join("home/.zshrc")).unwrap();
    assert!(profile.starts_with("alias ll='ls -l'\n"));
    assert_eq!(profile.matches(".local/bin:$PATH").count(), 1);
    assert!(profile.contains("# Added by the cua installer"));
}

#[cfg(unix)]
#[tokio::test]
async fn plan_reports_an_earlier_cua_on_path_that_would_shadow_the_install() {
    let tmp = tempfile::tempdir().unwrap();
    let cua = fake_cua(tmp.path(), VERSION);
    let other = tmp.path().join("other/bin");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("cua"), "#!/bin/sh\necho old\n").unwrap();
    let bin = tmp.path().join("home/.local/bin");
    let path_env = std::env::join_paths([other.clone(), bin]).unwrap();
    let cli = installer(tmp.path(), Some(cua), path_env.to_str().unwrap(), false);
    let plan = cli.plan().await;
    assert!(plan.on_path);
    assert_eq!(
        plan.shadowed_by.as_deref(),
        Some(other.join("cua").to_str().unwrap())
    );
}

#[tokio::test]
async fn install_without_a_bundled_cli_fails_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let cli = installer(tmp.path(), None, "", false);
    let plan = cli.plan().await;
    assert!(plan.source.is_none() && plan.method.is_none());
    let err = cli
        .install(&CliInstallRequest::default())
        .await
        .unwrap_err();
    assert!(err.contains("does not bundle"), "{err}");
    assert!(!tmp.path().join("home").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn cli_agent_setup_runs_the_bundled_cua_with_the_documented_args() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("args.log");
    let cua = fake_cua(
        tmp.path(),
        &format!(
            r#"echo "$HOME $*" >> '{log}'
case "$2" in
detect) echo '{{"agents":[{{"id":"claude-code","name":"Claude Code","installed":true,"mcp_config":"/h/.claude.json","cua_configured":false,"skills_installed":[]}},{{"id":"codex","name":"Codex","installed":false}}],"skills":[{{"name":"cua-driver","description":"Drive this computer","version":"1.0.0"}}]}}' ;;
setup) echo '[{{"agents":["claude-code"],"target":"mcp","item":"cua","path":"/h/.claude.json","change":"created","detail":"","backup":"/h/.claude.json.bak"}}]' ;;
esac"#,
            log = log.display()
        ),
    );
    let home = tmp.path().join("fakehome");
    let backend = CliAgentSetup::new(cua).with_env("HOME", home.display().to_string());
    let detected = backend.detect().await.unwrap();
    assert_eq!(detected.agents.len(), 2);
    assert!(detected.agents[0].installed);
    assert_eq!(
        detected.agents[0].mcp_config.as_deref(),
        Some("/h/.claude.json")
    );
    assert_eq!(detected.skills[0].name, "cua-driver");
    let report = backend
        .setup(AgentSetupRequest {
            agents: vec!["claude-code".into()],
            skills: false,
            mcp: true,
            driver: false,
        })
        .await
        .unwrap();
    assert_eq!(report.outcomes[0].change, "created");
    assert_eq!(
        report.outcomes[0].backup.as_deref(),
        Some("/h/.claude.json.bak")
    );
    let lines = std::fs::read_to_string(&log).unwrap();
    let lines: Vec<_> = lines.lines().collect();
    let home = home.display();
    assert_eq!(lines[0], format!("{home} agents detect --json"));
    assert_eq!(
        lines[1],
        format!("{home} agents setup --agents claude-code --no-skills --yes --json")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cli_agent_setup_runs_the_cua_driver_step_and_merges_its_outcomes() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("args.log");
    let cua = fake_cua(
        tmp.path(),
        &format!(
            r#"echo "$HOME $*" >> '{log}'
if [ "$3" = --cua-driver ]; then
  echo '{{"agents":["codex"],"server":{{"name":"cua-driver"}},"outcomes":[{{"agents":["codex"],"target":"skill","item":"cua-driver","path":"/h/.codex/skills/cua-driver","change":"created","detail":""}},{{"agents":["codex"],"target":"mcp","item":"cua-driver","path":"/h/.codex/config.toml","change":"created","detail":""}}]}}'
else
  echo '[{{"agents":["codex"],"target":"mcp","item":"cua","path":"/h/.codex/config.toml","change":"created","detail":""}}]'
fi"#,
            log = log.display()
        ),
    );
    let home = tmp.path().join("fakehome");
    let backend = CliAgentSetup::new(cua).with_env("HOME", home.display().to_string());
    let both = backend
        .setup(AgentSetupRequest {
            agents: vec!["codex".into()],
            skills: false,
            mcp: true,
            driver: true,
        })
        .await
        .unwrap();
    let items: Vec<_> = both
        .outcomes
        .iter()
        .map(|o| (o.target.as_str(), o.item.as_str()))
        .collect();
    assert_eq!(
        items,
        [
            ("mcp", "cua"),
            ("skill", "cua-driver"),
            ("mcp", "cua-driver")
        ]
    );
    let only = backend
        .setup(AgentSetupRequest {
            agents: vec!["codex".into()],
            skills: false,
            mcp: false,
            driver: true,
        })
        .await
        .unwrap();
    assert_eq!(only.outcomes.len(), 2);
    let lines = std::fs::read_to_string(&log).unwrap();
    let home = home.display();
    assert_eq!(
        lines.lines().collect::<Vec<_>>(),
        [
            format!("{home} agents setup --agents codex --no-skills --yes --json"),
            format!("{home} agents setup --cua-driver --agents codex --yes --json"),
            format!("{home} agents setup --cua-driver --agents codex --yes --json"),
        ]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn cli_agent_setup_surfaces_cli_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let cua = fake_cua(
        tmp.path(),
        "echo 'warming up' >&2; echo 'error: unknown agent kiro2' >&2; exit 2",
    );
    let err = CliAgentSetup::new(cua).detect().await.unwrap_err();
    assert_eq!(err, "error: unknown agent kiro2");
}

#[derive(Default)]
struct FakeAgents {
    calls: Mutex<Vec<AgentSetupRequest>>,
}

#[async_trait]
impl AgentSetupBackend for FakeAgents {
    async fn detect(&self) -> Result<AgentDetectReport, String> {
        Ok(AgentDetectReport {
            agents: vec![AgentInfo {
                id: "cursor".into(),
                name: "Cursor".into(),
                installed: true,
                ..Default::default()
            }],
            skills: vec![],
        })
    }
    async fn setup(&self, request: AgentSetupRequest) -> Result<AgentSetupReport, String> {
        self.calls.lock().unwrap().push(request);
        Ok(AgentSetupReport::default())
    }
}

#[tokio::test]
async fn commands_validate_setup_requests_before_the_backend_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let agents = Arc::new(FakeAgents::default());
    let commands = InstallerCommands::new(installer(tmp.path(), None, "", false), agents.clone());
    assert_eq!(
        commands.detect_agents().await.unwrap().agents[0].id,
        "cursor"
    );
    let err = commands
        .setup_agents(AgentSetupRequest {
            agents: vec![],
            skills: true,
            mcp: true,
            driver: false,
        })
        .await
        .unwrap_err();
    assert!(err.contains("at least one agent"));
    assert!(agents.calls.lock().unwrap().is_empty());
    commands
        .setup_agents(AgentSetupRequest {
            agents: vec!["cursor".into()],
            skills: true,
            mcp: true,
            driver: false,
        })
        .await
        .unwrap();
    assert_eq!(agents.calls.lock().unwrap().len(), 1);
    // cua-driver alone is enough to set up.
    commands
        .setup_agents(AgentSetupRequest {
            agents: vec!["cursor".into()],
            skills: false,
            mcp: false,
            driver: true,
        })
        .await
        .unwrap();
    assert!(agents.calls.lock().unwrap()[1].driver);
}
