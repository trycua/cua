// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-driver connect`: connect the agents on this computer to Cua Driver.
//!
//! Detects Claude Code, Codex and Cursor, registers one tool server named
//! `cua-driver` with each (through the agent's own CLI where it has one, by
//! merging into its JSON config otherwise) and installs the skill pack for
//! each. Running it twice changes nothing: a server already registered under
//! `cua-driver`, or under the old Claude Code name `cua-computer-use`, is left
//! alone. A failure for one agent never stops the others.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The one server name used for every new registration.
pub const SERVER_NAME: &str = "cua-driver";
/// The name older versions registered with Claude Code. Still recognised.
pub const LEGACY_CLAUDE_NAME: &str = "cua-computer-use";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Cursor,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::Claude, Agent::Codex, Agent::Cursor];

    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Cursor => "Cursor",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
        }
    }

    /// The `--client` value (and the `mcp-config` client) for this agent.
    pub fn parse(name: &str) -> Option<Agent> {
        match name {
            "claude" | "claude-code" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            "cursor" => Some(Agent::Cursor),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Connected,
    AlreadyConnected,
    WouldConnect,
    NotInstalled,
    Failed,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Connected => "connected",
            Status::AlreadyConnected => "already_connected",
            Status::WouldConnect => "would_connect",
            Status::NotInstalled => "not_installed",
            Status::Failed => "failed",
        }
    }
}

#[derive(Debug)]
pub struct AgentResult {
    pub agent: Agent,
    pub status: Status,
    pub detail: Option<String>,
}

impl AgentResult {
    fn new(agent: Agent, status: Status) -> Self {
        Self {
            agent,
            status,
            detail: None,
        }
    }
    fn failed(agent: Agent, why: impl Into<String>) -> Self {
        Self {
            agent,
            status: Status::Failed,
            detail: Some(why.into()),
        }
    }

    /// The one line a newcomer reads.
    pub fn line(&self) -> String {
        let label = self.agent.label();
        match self.status {
            Status::Connected => format!("{label}: connected"),
            Status::AlreadyConnected => format!("{label}: already connected"),
            Status::WouldConnect => format!("{label}: would connect"),
            Status::NotInstalled => format!("{label}: not installed"),
            Status::Failed => format!(
                "{label}: could not connect ({})",
                self.detail.as_deref().unwrap_or("unknown error")
            ),
        }
    }

    pub fn ok(&self) -> bool {
        matches!(
            self.status,
            Status::Connected | Status::AlreadyConnected | Status::WouldConnect
        )
    }
}

/// Everything that depends on the machine, so tests can point it at a temp
/// HOME and a temp PATH.
pub struct Env {
    pub home: PathBuf,
    pub path_dirs: Vec<PathBuf>,
    /// `$CLAUDE_CONFIG_DIR` when set, else `home`; holds `.claude.json`.
    pub claude_config_dir: PathBuf,
    /// `$CODEX_HOME` when set, else `home/.codex`.
    pub codex_home: PathBuf,
    /// Extra places an agent's app may live (e.g. `/Applications/Cursor.app`).
    pub app_dirs: Vec<PathBuf>,
}

impl Env {
    pub fn from_process() -> Option<Env> {
        #[cfg(windows)]
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from)?;
        #[cfg(not(windows))]
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        let path_dirs = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let nonempty = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let claude_config_dir = nonempty("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.clone());
        let codex_home = nonempty("CODEX_HOME").unwrap_or_else(|| home.join(".codex"));
        let mut app_dirs = vec![
            PathBuf::from("/Applications/Cursor.app"),
            home.join("Applications/Cursor.app"),
        ];
        if let Some(local) = nonempty("LOCALAPPDATA") {
            app_dirs.push(local.join("Programs").join("cursor"));
        }
        Some(Env {
            home,
            path_dirs,
            claude_config_dir,
            codex_home,
            app_dirs,
        })
    }

    fn find_on_path(&self, name: &str) -> Option<PathBuf> {
        let exts: &[&str] = if cfg!(windows) {
            &["", ".exe", ".cmd", ".bat"]
        } else {
            &[""]
        };
        for dir in &self.path_dirs {
            for ext in exts {
                let candidate = dir.join(format!("{name}{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        None
    }

    fn cursor_dir(&self) -> PathBuf {
        self.home.join(".cursor")
    }

    fn installed(&self, agent: Agent) -> Option<PathBuf> {
        match agent {
            Agent::Claude => self.find_on_path("claude"),
            Agent::Codex => self.find_on_path("codex"),
            Agent::Cursor => {
                if self.cursor_dir().is_dir() || self.app_dirs.iter().any(|p| p.exists()) {
                    Some(self.cursor_dir())
                } else {
                    self.find_on_path("cursor")
                }
            }
        }
    }
}

fn run_cli(program: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run {}: {e}", program.display()))?;
    if output.status.success() {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&output.stderr);
    let text = if text.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout)
    } else {
        text
    };
    let first = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("it exited with an error");
    Err(first.trim().to_owned())
}

// ── Claude Code ───────────────────────────────────────────────────────────

fn claude_registered(env: &Env) -> bool {
    let path = env.claude_config_dir.join(".claude.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    json.get("mcpServers")
        .and_then(|v| v.as_object())
        .is_some_and(|m| m.contains_key(SERVER_NAME) || m.contains_key(LEGACY_CLAUDE_NAME))
}

fn connect_claude(env: &Env, cli: &Path, bin: &str, dry_run: bool) -> AgentResult {
    if claude_registered(env) {
        return AgentResult::new(Agent::Claude, Status::AlreadyConnected);
    }
    if dry_run {
        return AgentResult::new(Agent::Claude, Status::WouldConnect);
    }
    let cfg = serde_json::json!({ "command": bin, "args": ["mcp"] }).to_string();
    match run_cli(
        cli,
        &["mcp", "add-json", "--scope", "user", SERVER_NAME, &cfg],
    ) {
        Ok(()) => AgentResult::new(Agent::Claude, Status::Connected),
        Err(e) if e.to_lowercase().contains("already exists") => {
            AgentResult::new(Agent::Claude, Status::AlreadyConnected)
        }
        Err(e) => AgentResult::failed(Agent::Claude, e),
    }
}

// ── Codex ─────────────────────────────────────────────────────────────────

fn codex_registered(env: &Env) -> bool {
    let Ok(text) = std::fs::read_to_string(env.codex_home.join("config.toml")) else {
        return false;
    };
    text.lines().any(|line| {
        let line = line.trim();
        [SERVER_NAME, LEGACY_CLAUDE_NAME].iter().any(|n| {
            line == format!("[mcp_servers.{n}]")
                || line == format!("[mcp_servers.\"{n}\"]")
                || line.starts_with(&format!("[mcp_servers.{n}."))
        })
    })
}

fn connect_codex(env: &Env, cli: &Path, bin: &str, dry_run: bool) -> AgentResult {
    if codex_registered(env) {
        return AgentResult::new(Agent::Codex, Status::AlreadyConnected);
    }
    if dry_run {
        return AgentResult::new(Agent::Codex, Status::WouldConnect);
    }
    match run_cli(cli, &["mcp", "add", SERVER_NAME, "--", bin, "mcp"]) {
        Ok(()) => AgentResult::new(Agent::Codex, Status::Connected),
        Err(e) => AgentResult::failed(Agent::Codex, e),
    }
}

// ── Cursor ────────────────────────────────────────────────────────────────

/// Add the server to Cursor's `mcp.json` text. Every other key and server is
/// kept. `Ok(None)` means it is already there; `Err` means the file is not a
/// JSON object we can safely edit.
pub fn merge_cursor_config(existing: Option<&str>, bin: &str) -> Result<Option<String>, String> {
    let mut root = match existing.map(str::trim).filter(|t| !t.is_empty()) {
        None => serde_json::json!({}),
        Some(text) => serde_json::from_str::<serde_json::Value>(text)
            .map_err(|e| format!("mcp.json is not valid JSON ({e}); left unchanged"))?,
    };
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "mcp.json is not a JSON object; left unchanged".to_owned())?;
    let servers = obj
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| "mcp.json has an unexpected mcpServers value; left unchanged".to_owned())?;
    if servers.contains_key(SERVER_NAME) {
        return Ok(None);
    }
    servers.insert(
        SERVER_NAME.to_owned(),
        serde_json::json!({ "command": bin, "args": ["mcp"], "type": "stdio" }),
    );
    let mut out = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(Some(out))
}

fn connect_cursor(env: &Env, bin: &str, dry_run: bool) -> AgentResult {
    let dir = env.cursor_dir();
    let path = dir.join("mcp.json");
    let existing = match std::fs::read_to_string(&path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return AgentResult::failed(Agent::Cursor, format!("could not read mcp.json: {e}"))
        }
    };
    let merged = match merge_cursor_config(existing.as_deref(), bin) {
        Ok(None) => return AgentResult::new(Agent::Cursor, Status::AlreadyConnected),
        Ok(Some(text)) => text,
        Err(e) => return AgentResult::failed(Agent::Cursor, e),
    };
    if dry_run {
        return AgentResult::new(Agent::Cursor, Status::WouldConnect);
    }
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("mcp.json.cua-driver-tmp");
        std::fs::write(&tmp, merged)?;
        std::fs::rename(&tmp, &path)
    };
    match write() {
        Ok(()) => AgentResult::new(Agent::Cursor, Status::Connected),
        Err(e) => AgentResult::failed(Agent::Cursor, format!("could not write mcp.json: {e}")),
    }
}

// ── Orchestration ─────────────────────────────────────────────────────────

/// Connect each of `agents` (all three when empty). Skill packs are not
/// handled here; `run_connect` adds them.
pub fn connect_agents(env: &Env, agents: &[Agent], bin: &str, dry_run: bool) -> Vec<AgentResult> {
    let list: Vec<Agent> = if agents.is_empty() {
        Agent::ALL.to_vec()
    } else {
        agents.to_vec()
    };
    list.into_iter()
        .map(|agent| {
            let Some(cli) = env.installed(agent) else {
                return AgentResult::new(agent, Status::NotInstalled);
            };
            match agent {
                Agent::Claude => connect_claude(env, &cli, bin, dry_run),
                Agent::Codex => connect_codex(env, &cli, bin, dry_run),
                Agent::Cursor => connect_cursor(env, bin, dry_run),
            }
        })
        .collect()
}

/// True when at least one agent was found and none of the found ones could be
/// connected. The only case `connect` exits non-zero for.
pub fn exit_failed(results: &[AgentResult]) -> bool {
    let found = results.iter().any(|r| r.status != Status::NotInstalled);
    found && !results.iter().any(AgentResult::ok)
}

fn stable_binary_path() -> String {
    // Prefer the path we were started by (the `~/.local/bin` link), which
    // survives upgrades, over the resolved per-version file.
    let argv0 = std::env::args().next().map(PathBuf::from);
    let path = match argv0 {
        Some(p) if p.is_absolute() && p.is_file() => p,
        _ => std::env::current_exe().unwrap_or_else(|_| PathBuf::from("cua-driver")),
    };
    path.to_string_lossy().replace('\\', "/")
}

pub fn run_connect(clients: &[String], dry_run: bool, json: bool) {
    let mut agents = Vec::new();
    for name in clients {
        match Agent::parse(name) {
            Some(a) if !agents.contains(&a) => agents.push(a),
            Some(_) => {}
            None => {
                eprintln!(
                    "cua-driver connect: unknown agent '{name}'. Use claude, codex or cursor, \
                     or `cua-driver mcp-config --client {name}` for other agents."
                );
                std::process::exit(2);
            }
        }
    }
    let Some(env) = Env::from_process() else {
        eprintln!("cua-driver connect: could not find your home folder.");
        std::process::exit(1);
    };
    let bin = stable_binary_path();
    let results = connect_agents(&env, &agents, &bin, dry_run);

    // Skill pack, for every agent that is now connected.
    let mut skills: Vec<(Agent, String)> = Vec::new();
    if !dry_run {
        for r in results.iter().filter(|r| r.ok()) {
            let note = match crate::skills::install_for_client(r.agent.id()) {
                Ok(crate::skills::ClientSkills::Installed(_)) => "installed",
                Ok(crate::skills::ClientSkills::NotFound(_)) => "folder_not_found",
                Ok(crate::skills::ClientSkills::Unsupported) => "unsupported",
                Err(_) => "failed",
            };
            skills.push((r.agent, note.to_owned()));
        }
    }
    let skill_of = |a: Agent| {
        skills
            .iter()
            .find(|(x, _)| *x == a)
            .map(|(_, s)| s.as_str())
    };

    if json {
        let items: Vec<_> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "agent": r.agent.id(),
                    "name": r.agent.label(),
                    "status": r.status.as_str(),
                    "detail": r.detail,
                    "skills": skill_of(r.agent),
                })
            })
            .collect();
        let out = serde_json::json!({
            "server_name": SERVER_NAME,
            "dry_run": dry_run,
            "agents": items,
            "ok": !exit_failed(&results),
        });
        println!("{out}");
    } else {
        for r in &results {
            println!("{}", r.line());
        }
        if results.iter().all(|r| r.status == Status::NotInstalled) {
            println!("Cua Driver is installed. When you install Claude Code, Codex or Cursor, run: cua-driver connect");
        }
    }
    if exit_failed(&results) {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg_attr(not(unix), allow(dead_code))]
    struct Fixture {
        _dir: tempfile::TempDir,
        env: Env,
        bin_dir: PathBuf,
        log: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&bin_dir).unwrap();
        let log = dir.path().join("calls.log");
        let env = Env {
            claude_config_dir: home.clone(),
            codex_home: home.join(".codex"),
            home,
            path_dirs: vec![bin_dir.clone()],
            app_dirs: vec![],
        };
        Fixture {
            _dir: dir,
            env,
            bin_dir,
            log,
        }
    }

    #[cfg(unix)]
    fn fake_cli(f: &Fixture, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let path = f.bin_dir.join(name);
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"{name} $@\" >> '{}'\n{body}\n",
                f.log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    fn calls(f: &Fixture) -> String {
        std::fs::read_to_string(&f.log).unwrap_or_default()
    }

    fn by(results: &[AgentResult], a: Agent) -> &AgentResult {
        results.iter().find(|r| r.agent == a).unwrap()
    }

    #[test]
    fn nothing_installed_reports_not_installed_and_does_not_fail() {
        let f = fixture();
        let r = connect_agents(&f.env, &[], "/b/cua-driver", false);
        assert!(r.iter().all(|r| r.status == Status::NotInstalled));
        assert_eq!(by(&r, Agent::Codex).line(), "Codex: not installed");
        assert!(!exit_failed(&r));
    }

    #[cfg(unix)]
    #[test]
    fn detects_claude_and_codex_on_path_and_cursor_by_folder() {
        let f = fixture();
        fake_cli(&f, "claude", "exit 0");
        fake_cli(&f, "codex", "exit 0");
        std::fs::create_dir_all(f.env.home.join(".cursor")).unwrap();
        let r = connect_agents(&f.env, &[], "/b/cua-driver", false);
        assert_eq!(by(&r, Agent::Claude).status, Status::Connected);
        assert_eq!(by(&r, Agent::Codex).status, Status::Connected);
        assert_eq!(by(&r, Agent::Cursor).status, Status::Connected);
        let log = calls(&f);
        assert!(log.contains(
            "claude mcp add-json --scope user cua-driver {\"args\":[\"mcp\"],\"command\":\"/b/cua-driver\"}"
        ), "{log}");
        assert!(
            log.contains("codex mcp add cua-driver -- /b/cua-driver mcp"),
            "{log}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn registered_under_current_or_legacy_name_is_not_added_again() {
        let f = fixture();
        fake_cli(&f, "claude", "exit 0");
        fake_cli(&f, "codex", "exit 0");
        for name in [SERVER_NAME, LEGACY_CLAUDE_NAME] {
            std::fs::write(
                f.env.home.join(".claude.json"),
                format!("{{\"mcpServers\":{{\"{name}\":{{}}}}}}"),
            )
            .unwrap();
            let r = connect_agents(&f.env, &[Agent::Claude], "/b/cua-driver", false);
            assert_eq!(
                by(&r, Agent::Claude).line(),
                "Claude Code: already connected"
            );
        }
        std::fs::create_dir_all(&f.env.codex_home).unwrap();
        std::fs::write(
            f.env.codex_home.join("config.toml"),
            "[mcp_servers.cua-driver]\ncommand = \"x\"\n",
        )
        .unwrap();
        let r = connect_agents(&f.env, &[Agent::Codex], "/b/cua-driver", false);
        assert_eq!(by(&r, Agent::Codex).status, Status::AlreadyConnected);
        assert!(!calls(&f).contains("add"), "{}", calls(&f));
    }

    #[cfg(unix)]
    #[test]
    fn second_run_is_idempotent_for_cursor_and_claude_already_exists() {
        let f = fixture();
        fake_cli(
            &f,
            "claude",
            "echo 'MCP server cua-driver already exists' >&2; exit 1",
        );
        let r = connect_agents(&f.env, &[Agent::Claude], "/b/cua-driver", false);
        assert_eq!(by(&r, Agent::Claude).status, Status::AlreadyConnected);
        std::fs::create_dir_all(f.env.home.join(".cursor")).unwrap();
        let first = connect_agents(&f.env, &[Agent::Cursor], "/b/cua-driver", false);
        assert_eq!(by(&first, Agent::Cursor).status, Status::Connected);
        let second = connect_agents(&f.env, &[Agent::Cursor], "/b/cua-driver", false);
        assert_eq!(by(&second, Agent::Cursor).status, Status::AlreadyConnected);
    }

    #[test]
    fn cursor_merge_keeps_other_servers_and_keys() {
        let existing = r#"{"theme":"x","mcpServers":{"other":{"command":"o","args":[]}}}"#;
        let merged = merge_cursor_config(Some(existing), "/b/cua-driver")
            .unwrap()
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["theme"], "x");
        assert_eq!(v["mcpServers"]["other"]["command"], "o");
        assert_eq!(v["mcpServers"]["cua-driver"]["command"], "/b/cua-driver");
        assert_eq!(v["mcpServers"]["cua-driver"]["args"][0], "mcp");
        assert_eq!(
            merge_cursor_config(Some(&merged), "/b/cua-driver").unwrap(),
            None
        );
        assert!(merge_cursor_config(None, "/b")
            .unwrap()
            .unwrap()
            .contains("cua-driver"));
    }

    #[test]
    fn cursor_invalid_json_is_never_overwritten() {
        let f = fixture();
        let dir = f.env.home.join(".cursor");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mcp.json"), "{ not json").unwrap();
        let r = connect_agents(&f.env, &[Agent::Cursor], "/b", false);
        assert_eq!(by(&r, Agent::Cursor).status, Status::Failed);
        assert_eq!(
            std::fs::read_to_string(dir.join("mcp.json")).unwrap(),
            "{ not json"
        );
        assert!(exit_failed(&r));
    }

    #[cfg(unix)]
    #[test]
    fn dry_run_changes_nothing() {
        let f = fixture();
        fake_cli(&f, "claude", "exit 0");
        fake_cli(&f, "codex", "exit 0");
        std::fs::create_dir_all(f.env.home.join(".cursor")).unwrap();
        let r = connect_agents(&f.env, &[], "/b/cua-driver", true);
        assert!(r.iter().all(|r| r.status == Status::WouldConnect));
        assert_eq!(calls(&f), "");
        assert!(!f.env.home.join(".cursor/mcp.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn one_failure_does_not_fail_the_others_or_the_exit_code() {
        let f = fixture();
        fake_cli(&f, "claude", "echo boom >&2; exit 1");
        fake_cli(&f, "codex", "exit 0");
        let r = connect_agents(&f.env, &[], "/b/cua-driver", false);
        assert_eq!(
            by(&r, Agent::Claude).line(),
            "Claude Code: could not connect (boom)"
        );
        assert_eq!(by(&r, Agent::Codex).status, Status::Connected);
        assert!(!exit_failed(&r));
    }

    #[cfg(unix)]
    #[test]
    fn exits_non_zero_only_when_agents_were_found_and_none_connected() {
        let f = fixture();
        fake_cli(&f, "claude", "echo boom >&2; exit 1");
        let r = connect_agents(&f.env, &[], "/b/cua-driver", false);
        assert!(exit_failed(&r));
    }

    #[cfg(unix)]
    #[test]
    fn client_filter_limits_the_agents() {
        let f = fixture();
        fake_cli(&f, "claude", "exit 0");
        fake_cli(&f, "codex", "exit 0");
        let r = connect_agents(&f.env, &[Agent::Codex], "/b/cua-driver", false);
        assert_eq!(r.len(), 1);
        assert!(!calls(&f).contains("claude"));
    }
}
