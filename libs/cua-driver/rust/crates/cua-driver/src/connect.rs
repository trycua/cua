// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `cua-driver connect`: connect the agents on this computer to Cua Driver.
//!
//! Detects Claude Code, Codex, Cursor and Factory Droid, registers one tool
//! server named `cua-driver` with each (through the agent's own CLI where it
//! has one, by merging into its JSON config otherwise) and installs the skill
//! pack for each that has a skills folder. Running it twice changes nothing: a server already registered under
//! `cua-driver`, or under the old Claude Code name `cua-computer-use`, is left
//! alone. When that server launches a different Cua Driver (Cua Spaces' Set up
//! writes the same name for the one it carries), the agent is reported as
//! using another Cua Driver instead of as connected; `--replace` switches it
//! to this one through the step Set up runs. A failure for one agent never
//! stops the others.

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
    Droid,
}

impl Agent {
    pub const ALL: [Agent; 4] = [Agent::Claude, Agent::Codex, Agent::Cursor, Agent::Droid];

    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
            Agent::Cursor => "Cursor",
            Agent::Droid => "Factory Droid",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Cursor => "cursor",
            Agent::Droid => "droid",
        }
    }

    /// The agent's id in `cua-agent-setup`, whose `configure_mcp` Cua Spaces'
    /// Set up runs. Droid is not in its registry.
    fn setup_id(self) -> Option<&'static str> {
        match self {
            Agent::Claude => Some("claude-code"),
            Agent::Codex => Some("codex"),
            Agent::Cursor => Some("cursor"),
            Agent::Droid => None,
        }
    }

    /// The `--client` value (and the `mcp-config` client) for this agent.
    pub fn parse(name: &str) -> Option<Agent> {
        match name {
            "claude" | "claude-code" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            "cursor" => Some(Agent::Cursor),
            "droid" | "factory" => Some(Agent::Droid),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Connected,
    AlreadyConnected,
    /// The agent's `cua-driver` server launches a different Cua Driver.
    OtherDriver,
    /// That server now launches this driver (`--replace`).
    Replaced,
    WouldReplace,
    WouldConnect,
    NotInstalled,
    Failed,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Connected => "connected",
            Status::AlreadyConnected => "already_connected",
            Status::OtherDriver => "other_driver",
            Status::Replaced => "replaced",
            Status::WouldReplace => "would_replace",
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
    /// The name the agent lists the server under.
    pub server: &'static str,
}

impl AgentResult {
    fn new(agent: Agent, status: Status) -> Self {
        Self {
            agent,
            status,
            detail: None,
            server: SERVER_NAME,
        }
    }
    fn failed(agent: Agent, why: impl Into<String>) -> Self {
        Self {
            agent,
            status: Status::Failed,
            detail: Some(why.into()),
            server: SERVER_NAME,
        }
    }

    /// The one line a newcomer reads.
    pub fn line(&self) -> String {
        let label = self.agent.label();
        match self.status {
            Status::Connected => format!("{label}: connected"),
            Status::AlreadyConnected => format!("{label}: already connected"),
            Status::OtherDriver if self.replaceable() => format!(
                "{label}: uses another Cua Driver ({}); left as is. To use this one, run \
                 `cua-driver connect --replace`.",
                self.detail.as_deref().unwrap_or("unknown")
            ),
            Status::OtherDriver => format!(
                "{label}: uses another Cua Driver ({}); left as is. To use this one, remove its \
                 `{}` server and run `cua-driver connect` again.",
                self.detail.as_deref().unwrap_or("unknown"),
                self.server
            ),
            Status::Replaced => format!(
                "{label}: switched to this Cua Driver from {}",
                self.detail.as_deref().unwrap_or("another one")
            ),
            Status::WouldReplace => format!(
                "{label}: would switch to this Cua Driver from {}",
                self.detail.as_deref().unwrap_or("another one")
            ),
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
            Status::Connected
                | Status::AlreadyConnected
                | Status::OtherDriver
                | Status::Replaced
                | Status::WouldReplace
                | Status::WouldConnect
        )
    }

    /// Whether `--replace` can switch this agent: Set up's step knows it and
    /// the server has the name that step writes (a legacy-named one would be
    /// left beside the new entry).
    fn replaceable(&self) -> bool {
        self.agent.setup_id().is_some() && self.server == SERVER_NAME
    }

    /// Points the agent's `cua-driver` server at `bin` with
    /// the writer Cua Spaces' Set up runs (`AgentSetup::configure_mcp`): the
    /// agent's own CLI where it has one, a format-preserving edit otherwise,
    /// and a backup of the file first. Unlike Set up it leaves a retired
    /// `cua-spaces` entry alone. Anything but a replaceable other driver is
    /// returned as is.
    fn replace(self, env: &Env, bin: &str, dry_run: bool) -> AgentResult {
        let id = match self.agent.setup_id() {
            Some(id) if self.status == Status::OtherDriver && self.replaceable() => id,
            _ => return self,
        };
        if dry_run {
            return AgentResult {
                status: Status::WouldReplace,
                ..self
            };
        }
        let setup = cua_agent_setup::AgentSetup::new(env.setup_env());
        let server = cua_agent_setup::McpServer::driver(bin);
        let written = setup
            .configure_mcp_only(&[id.to_owned()], &server)
            .map_err(|e| e.to_string())
            .and_then(|out| {
                out.into_iter()
                    .find(|o| o.item == SERVER_NAME)
                    .ok_or_else(|| "nothing was written".to_owned())
            });
        match written {
            Ok(o) if o.failed() => AgentResult::failed(self.agent, o.detail),
            Ok(_) => AgentResult {
                status: Status::Replaced,
                ..self
            },
            Err(e) => AgentResult::failed(self.agent, e),
        }
    }
}

/// The server an agent already has under one of our names, and the command
/// it launches when the entry names one.
struct Existing {
    server: &'static str,
    command: Option<String>,
}

impl Existing {
    /// The server in an `mcpServers` map (Claude Code, Cursor, Droid).
    fn in_json(text: &str, names: &[&'static str]) -> Option<Existing> {
        let json = serde_json::from_str::<serde_json::Value>(text).ok()?;
        let servers = json.get("mcpServers")?.as_object()?;
        let (server, entry) = names
            .iter()
            .find_map(|name| Some((*name, servers.get(*name)?)))?;
        let command = entry.get("command").and_then(|c| c.as_str());
        Some(Existing {
            server,
            command: command.map(str::to_owned),
        })
    }

    /// The `[mcp_servers.<name>]` table in Codex's `config.toml`. `Err` when
    /// the file is not TOML, so nothing is registered into a file we cannot
    /// read.
    fn in_toml(text: &str, names: &[&'static str]) -> Result<Option<Existing>, String> {
        let config = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|_| "config.toml is not valid TOML; left unchanged".to_owned())?;
        let servers = config.get("mcp_servers").and_then(|s| s.as_table_like());
        Ok(names.iter().find_map(|name| {
            let command = servers?.get(name)?.get("command");
            Some(Existing {
                server: name,
                command: command.and_then(|c| c.as_str()).map(str::to_owned),
            })
        }))
    }

    /// Already connected, unless the entry names an absolute path that is a
    /// different file from `bin` once links are resolved (`~/.local/bin`
    /// links to the installed driver). An entry that names no path, or a
    /// `bin` we could not resolve, is not evidence of another driver.
    fn result(self, agent: Agent, bin: &str) -> AgentResult {
        let resolved = |path: &str| std::fs::canonicalize(path).unwrap_or_else(|_| path.into());
        let other = self.command.filter(|command| {
            Path::new(command).is_absolute()
                && Path::new(bin).is_absolute()
                && resolved(command) != resolved(bin)
        });
        AgentResult {
            agent,
            status: match other {
                Some(_) => Status::OtherDriver,
                None => Status::AlreadyConnected,
            },
            detail: other,
            server: self.server,
        }
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
    /// `$CUA_HOME` when set, where `--replace` keeps Set up's record.
    pub cua_home: Option<PathBuf>,
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
            cua_home: nonempty("CUA_HOME"),
        })
    }

    /// The same machine as `cua-agent-setup` sees it, for `--replace`.
    fn setup_env(&self) -> cua_agent_setup::HostEnv {
        let mut host = cua_agent_setup::HostEnv::isolated(&self.home);
        host.path = self.path_dirs.clone();
        host.run_agent_clis = true;
        let dirs = [
            ("CLAUDE_CONFIG_DIR", Some(&self.claude_config_dir)),
            ("CODEX_HOME", Some(&self.codex_home)),
            ("CUA_HOME", self.cua_home.as_ref()),
        ];
        for (key, dir) in dirs {
            if let Some(dir) = dir {
                host.vars.insert(key.into(), dir.display().to_string());
            }
        }
        host
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

    fn factory_dir(&self) -> PathBuf {
        self.home.join(".factory")
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
            Agent::Droid => self
                .find_on_path("droid")
                .or_else(|| self.factory_dir().is_dir().then(|| self.factory_dir())),
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

fn claude_existing(env: &Env) -> Option<Existing> {
    let text = std::fs::read_to_string(env.claude_config_dir.join(".claude.json")).ok()?;
    Existing::in_json(&text, &[SERVER_NAME, LEGACY_CLAUDE_NAME])
}

fn connect_claude(env: &Env, cli: &Path, bin: &str, dry_run: bool) -> AgentResult {
    if let Some(existing) = claude_existing(env) {
        return existing.result(Agent::Claude, bin);
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

fn codex_existing(env: &Env) -> Result<Option<Existing>, String> {
    match std::fs::read_to_string(env.codex_home.join("config.toml")) {
        Ok(text) => Existing::in_toml(&text, &[SERVER_NAME, LEGACY_CLAUDE_NAME]),
        Err(_) => Ok(None),
    }
}

fn connect_codex(env: &Env, cli: &Path, bin: &str, dry_run: bool) -> AgentResult {
    match codex_existing(env) {
        Ok(None) => {}
        Ok(Some(existing)) => return existing.result(Agent::Codex, bin),
        Err(e) => return AgentResult::failed(Agent::Codex, e),
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

/// Add the server to an `mcp.json` text with a top-level `mcpServers` map
/// (Cursor and Droid). Every other key and server is kept. `Ok(None)` means it
/// is already there; `Err` means the file is not a JSON object we can safely
/// edit.
pub fn merge_mcp_json(existing: Option<&str>, bin: &str) -> Result<Option<String>, String> {
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

/// The result for an `mcp.json` that already lists the server.
fn already(agent: Agent, mcp_json: Option<&str>, bin: &str) -> AgentResult {
    mcp_json
        .and_then(|text| Existing::in_json(text, &[SERVER_NAME]))
        .map_or_else(
            || AgentResult::new(agent, Status::AlreadyConnected),
            |existing| existing.result(agent, bin),
        )
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
    let merged = match merge_mcp_json(existing.as_deref(), bin) {
        Ok(None) => return already(Agent::Cursor, existing.as_deref(), bin),
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

// ── Factory Droid ─────────────────────────────────────────────────────────

/// Tools Droid keeps at its own default (`high`) because they cannot be
/// undone. Every other tool without `readOnlyHint` is rated `medium`.
const DROID_KEEP_DEFAULT: &[&str] = &["kill_app"];

/// Droid rates an MCP tool `low` when it declares `readOnlyHint` and `high`
/// otherwise, so every click or keystroke needs `--auto high`, while the same
/// actions run as `cua-driver call` commands are rated `medium`. Add a
/// `medium` rating for each such tool to `settings.json`'s
/// `mcpAutonomyOverrides`, keeping every value already there. `Ok(None)`
/// means nothing was missing.
pub fn merge_droid_settings(
    existing: Option<&str>,
    tools: &serde_json::Value,
) -> Result<Option<String>, String> {
    let mut root = match existing.map(str::trim).filter(|t| !t.is_empty()) {
        None => serde_json::json!({}),
        Some(text) => serde_json::from_str::<serde_json::Value>(text)
            .map_err(|e| format!("settings.json is not valid JSON ({e}); left unchanged"))?,
    };
    let rated = root
        .as_object_mut()
        .and_then(|o| {
            o.entry("mcpAutonomyOverrides")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
        })
        .and_then(|o| {
            o.entry(SERVER_NAME)
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
        })
        .and_then(|o| {
            o.entry("tools")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
        })
        .ok_or_else(|| "settings.json has an unexpected shape; left unchanged".to_owned())?;
    let mut added = false;
    for tool in tools
        .get("tools")
        .and_then(|t| t.as_array())
        .into_iter()
        .flatten()
    {
        let Some(name) = tool.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let read_only =
            tool.pointer("/annotations/readOnlyHint") == Some(&serde_json::Value::Bool(true));
        if read_only || DROID_KEEP_DEFAULT.contains(&name) || rated.contains_key(name) {
            continue;
        }
        rated.insert(name.to_owned(), serde_json::json!("medium"));
        added = true;
    }
    if !added {
        return Ok(None);
    }
    let mut out = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(Some(out))
}

fn connect_droid(env: &Env, bin: &str, tools: &serde_json::Value, dry_run: bool) -> AgentResult {
    let dir = env.factory_dir();
    let read = |name: &str| match std::fs::read_to_string(dir.join(name)) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not read {name}: {e}")),
    };
    let mcp_json = match read("mcp.json") {
        Ok(text) => text,
        Err(e) => return AgentResult::failed(Agent::Droid, e),
    };
    // Another driver's server is left as is, ratings included.
    let listed = mcp_json
        .as_deref()
        .and_then(|text| Existing::in_json(text, &[SERVER_NAME]))
        .map(|existing| existing.result(Agent::Droid, bin));
    if let Some(other) = listed.filter(|r| r.status == Status::OtherDriver) {
        return other;
    }
    let merged = merge_mcp_json(mcp_json.as_deref(), bin).and_then(|mcp| {
        read("settings.json")
            .and_then(|t| merge_droid_settings(t.as_deref(), tools))
            .map(|settings| [("mcp.json", mcp), ("settings.json", settings)])
    });
    let writes = match merged {
        Ok(files) => files,
        Err(e) => return AgentResult::failed(Agent::Droid, e),
    };
    if writes.iter().all(|(_, text)| text.is_none()) {
        return AgentResult::new(Agent::Droid, Status::AlreadyConnected);
    }
    if dry_run {
        return AgentResult::new(Agent::Droid, Status::WouldConnect);
    }
    for (name, text) in writes {
        let Some(text) = text else { continue };
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            let tmp = dir.join(format!("{name}.cua-driver-tmp"));
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, dir.join(name))
        };
        if let Err(e) = write() {
            return AgentResult::failed(Agent::Droid, format!("could not write {name}: {e}"));
        }
    }
    AgentResult::new(Agent::Droid, Status::Connected)
}

// ── Orchestration ─────────────────────────────────────────────────────────

/// Connect each of `agents` (every known agent when empty). `tools` is the
/// driver's tool inventory, read for Droid's autonomy ratings. With `replace`,
/// an agent that uses another Cua Driver is switched to this one. Skill packs
/// are not handled here; `run_connect` adds them.
pub fn connect_agents(
    env: &Env,
    agents: &[Agent],
    bin: &str,
    tools: &serde_json::Value,
    dry_run: bool,
    replace: bool,
) -> Vec<AgentResult> {
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
            let result = match agent {
                Agent::Claude => connect_claude(env, &cli, bin, dry_run),
                Agent::Codex => connect_codex(env, &cli, bin, dry_run),
                Agent::Cursor => connect_cursor(env, bin, dry_run),
                Agent::Droid => connect_droid(env, bin, tools, dry_run),
            };
            if replace {
                result.replace(env, bin, dry_run)
            } else {
                result
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

pub fn run_connect(
    clients: &[String],
    tools: &serde_json::Value,
    dry_run: bool,
    replace: bool,
    json: bool,
) {
    let mut agents = Vec::new();
    for name in clients {
        match Agent::parse(name) {
            Some(a) if !agents.contains(&a) => agents.push(a),
            Some(_) => {}
            None => {
                eprintln!(
                    "cua-driver connect: unknown agent '{name}'. Use claude, codex, cursor or droid, \
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
    let results = connect_agents(&env, &agents, &bin, tools, dry_run, replace);

    // Skill pack, for every agent that is now connected to this driver.
    let mut skills: Vec<(Agent, String)> = Vec::new();
    if !dry_run {
        for r in results
            .iter()
            .filter(|r| r.ok() && r.status != Status::OtherDriver)
        {
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
            println!("Cua Driver is installed. When you install Claude Code, Codex, Cursor or Factory Droid, run: cua-driver connect");
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
            cua_home: None,
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

    fn no_tools() -> serde_json::Value {
        serde_json::json!({ "tools": [] })
    }

    fn by(results: &[AgentResult], a: Agent) -> &AgentResult {
        results.iter().find(|r| r.agent == a).unwrap()
    }

    #[test]
    fn nothing_installed_reports_not_installed_and_does_not_fail() {
        let f = fixture();
        let r = connect_agents(&f.env, &[], "/b/cua-driver", &no_tools(), false, false);
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
        let r = connect_agents(&f.env, &[], "/b/cua-driver", &no_tools(), false, false);
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
            let r = connect_agents(
                &f.env,
                &[Agent::Claude],
                "/b/cua-driver",
                &no_tools(),
                false,
                false,
            );
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
        let r = connect_agents(
            &f.env,
            &[Agent::Codex],
            "/b/cua-driver",
            &no_tools(),
            false,
            false,
        );
        assert_eq!(by(&r, Agent::Codex).status, Status::AlreadyConnected);
        assert!(!calls(&f).contains("add"), "{}", calls(&f));
    }

    /// What Cua Spaces' Set up writes: the driver it carries, by absolute
    /// path, under the same server name.
    #[cfg(unix)]
    const SPACES_DRIVER: &str = "/Applications/Cua Spaces.app/Contents/Helpers/cua-driver";

    #[cfg(unix)]
    #[test]
    fn a_server_that_launches_another_driver_is_reported_and_left_alone() {
        let f = fixture();
        fake_cli(&f, "claude", "exit 0");
        fake_cli(&f, "codex", "exit 0");
        let ours = f.bin_dir.join("cua-driver");
        std::fs::write(&ours, "").unwrap();
        let ours = ours.to_str().unwrap();
        let entry = serde_json::json!({ "mcpServers": { "cua-driver": {
            "command": SPACES_DRIVER, "args": ["mcp", "--embedded"] } } })
        .to_string();
        std::fs::write(f.env.home.join(".claude.json"), &entry).unwrap();
        std::fs::create_dir_all(&f.env.codex_home).unwrap();
        let toml =
            format!("[mcp_servers.cua-driver]\ncommand = \"{SPACES_DRIVER}\"\nargs = [\"mcp\"]\n");
        std::fs::write(f.env.codex_home.join("config.toml"), &toml).unwrap();
        for dir in [".cursor", ".factory"] {
            std::fs::create_dir_all(f.env.home.join(dir)).unwrap();
            std::fs::write(f.env.home.join(dir).join("mcp.json"), &entry).unwrap();
        }
        for dry_run in [true, false] {
            let r = connect_agents(&f.env, &[], ours, &droid_tools(), dry_run, false);
            for agent in Agent::ALL {
                let result = by(&r, agent);
                assert_eq!(result.status, Status::OtherDriver, "{agent:?}");
                assert_eq!(result.detail.as_deref(), Some(SPACES_DRIVER));
                assert!(result.ok());
            }
        }
        assert_eq!(
            by(
                &connect_agents(&f.env, &[Agent::Codex], ours, &no_tools(), false, false),
                Agent::Codex
            )
            .line(),
            format!(
                "Codex: uses another Cua Driver ({SPACES_DRIVER}); left as is. To use this one, \
                 run `cua-driver connect --replace`."
            )
        );
        assert!(
            by(
                &connect_agents(&f.env, &[Agent::Droid], ours, &droid_tools(), false, false),
                Agent::Droid
            )
            .line()
            .ends_with("remove its `cua-driver` server and run `cua-driver connect` again."),
            "Set up's step does not know Droid, so --replace is not offered"
        );
        assert!(!calls(&f).contains("add"), "{}", calls(&f));
        for path in [".claude.json", ".cursor/mcp.json", ".factory/mcp.json"] {
            assert_eq!(
                std::fs::read_to_string(f.env.home.join(path)).unwrap(),
                entry
            );
        }
        assert!(!f.env.home.join(".factory/settings.json").exists());
        assert_eq!(
            std::fs::read_to_string(f.env.codex_home.join("config.toml")).unwrap(),
            toml
        );
    }

    #[cfg(unix)]
    #[test]
    fn replace_switches_an_agent_that_uses_another_driver_to_this_one() {
        let f = fixture();
        // `claude mcp add-json --scope user <name> <json>` writes the entry.
        let claude_json = f.env.home.join(".claude.json");
        fake_cli(
            &f,
            "claude",
            &format!(
                "[ \"$2\" = add-json ] && printf '{{\"mcpServers\":{{\"%s\":%s}}}}' \"$5\" \"$6\" > '{}'\nexit 0",
                claude_json.display()
            ),
        );
        fake_cli(&f, "codex", "exit 0");
        let ours = f.bin_dir.join("cua-driver");
        std::fs::write(&ours, "").unwrap();
        let ours = ours.to_str().unwrap();
        let entry = serde_json::json!({ "mcpServers": {
            "cua-driver": { "command": SPACES_DRIVER, "args": ["mcp", "--embedded"] },
            "cua-spaces": { "command": "python3", "args": ["/x/spaces_mcp.py"] },
            "other": { "command": "/x/other" } } })
        .to_string();
        std::fs::write(&claude_json, &entry).unwrap();
        std::fs::create_dir_all(&f.env.codex_home).unwrap();
        let toml = format!(
            "# mine\nmodel = \"o3\"\n\n[mcp_servers.cua-driver]\ncommand = \"{SPACES_DRIVER}\"\nargs = [\"mcp\"]\n"
        );
        std::fs::write(f.env.codex_home.join("config.toml"), &toml).unwrap();
        for dir in [".cursor", ".factory"] {
            std::fs::create_dir_all(f.env.home.join(dir)).unwrap();
            std::fs::write(f.env.home.join(dir).join("mcp.json"), &entry).unwrap();
        }

        let preview = connect_agents(&f.env, &[], ours, &droid_tools(), true, true);
        assert_eq!(
            by(&preview, Agent::Codex).line(),
            format!("Codex: would switch to this Cua Driver from {SPACES_DRIVER}")
        );
        assert_eq!(std::fs::read_to_string(&claude_json).unwrap(), entry);

        let r = connect_agents(&f.env, &[], ours, &droid_tools(), false, true);
        for agent in [Agent::Claude, Agent::Codex, Agent::Cursor] {
            assert_eq!(
                by(&r, agent).line(),
                format!(
                    "{}: switched to this Cua Driver from {SPACES_DRIVER}",
                    agent.label()
                )
            );
        }
        assert_eq!(by(&r, Agent::Droid).status, Status::OtherDriver);
        let command = |path: &Path| {
            let v: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            v["mcpServers"]["cua-driver"]["command"].clone()
        };
        assert_eq!(command(&claude_json), ours);
        assert_eq!(command(&f.env.home.join(".cursor/mcp.json")), ours);
        assert_eq!(
            command(&f.env.home.join(".factory/mcp.json")),
            SPACES_DRIVER
        );
        let cursor: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(f.env.home.join(".cursor/mcp.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cursor["mcpServers"]["other"]["command"], "/x/other");
        // Unlike Set up, `--replace` leaves the retired `cua-spaces` entry.
        assert_eq!(cursor["mcpServers"]["cua-spaces"]["command"], "python3");
        let codex = std::fs::read_to_string(f.env.codex_home.join("config.toml")).unwrap();
        assert!(codex.starts_with("# mine\nmodel = \"o3\"\n"), "{codex}");
        assert!(codex.contains(&format!("command = \"{ours}\"")), "{codex}");
        // Set up's bookkeeping: a backup of each file before it is written.
        let backups = |dir: &Path, name: &str| {
            std::fs::read_dir(dir)
                .unwrap()
                .filter(|e| {
                    let file = e.as_ref().unwrap().file_name();
                    file.to_string_lossy()
                        .starts_with(&format!("{name}.cua-backup-"))
                })
                .count()
        };
        assert_eq!(backups(&f.env.home, ".claude.json"), 1);
        assert_eq!(backups(&f.env.codex_home, "config.toml"), 1);
        assert_eq!(backups(&f.env.home.join(".cursor"), "mcp.json"), 1);

        let again = connect_agents(&f.env, &[], ours, &droid_tools(), false, true);
        for agent in [Agent::Claude, Agent::Codex, Agent::Cursor] {
            assert_eq!(by(&again, agent).status, Status::AlreadyConnected);
        }
    }

    #[cfg(unix)]
    #[test]
    fn codex_config_is_read_as_toml_and_the_listed_name_is_the_one_to_remove() {
        let f = fixture();
        fake_cli(&f, "claude", "exit 0");
        fake_cli(&f, "codex", "exit 0");
        let ours = f.bin_dir.join("cua-driver");
        std::fs::write(&ours, "").unwrap();
        let ours = ours.to_str().unwrap();
        std::fs::create_dir_all(&f.env.codex_home).unwrap();
        for toml in [
            format!("[mcp_servers.cua-driver] # Cua Spaces\ncommand = \"{SPACES_DRIVER}\" # theirs\n"),
            format!("[mcp_servers.cua-driver.env]\nA = \"b\"\n\n[mcp_servers.cua-driver]\ncommand = '{SPACES_DRIVER}'\n"),
            format!("[mcp_servers.\"cua-driver\"]\nargs = [\"mcp\"]\ncommand = \"{SPACES_DRIVER}\"\n"),
            format!("mcp_servers = {{ cua-driver = {{ command = \"{SPACES_DRIVER}\" }} }}\n"),
        ] {
            std::fs::write(f.env.codex_home.join("config.toml"), &toml).unwrap();
            let r = connect_agents(&f.env, &[Agent::Codex], ours, &no_tools(), false, false);
            assert_eq!(by(&r, Agent::Codex).status, Status::OtherDriver, "{toml}");
        }
        std::fs::write(
            f.env.codex_home.join("config.toml"),
            "[mcp_servers.other]\ncommand = \"/x/cua-driver\"\n",
        )
        .unwrap();
        let r = connect_agents(&f.env, &[Agent::Codex], ours, &no_tools(), true, false);
        assert_eq!(by(&r, Agent::Codex).status, Status::WouldConnect);

        let broken = "[mcp_servers.cua-driver]\ncommand = \"/x\n";
        std::fs::write(f.env.codex_home.join("config.toml"), broken).unwrap();
        let r = connect_agents(&f.env, &[Agent::Codex], ours, &no_tools(), false, false);
        assert_eq!(
            by(&r, Agent::Codex).line(),
            "Codex: could not connect (config.toml is not valid TOML; left unchanged)"
        );

        std::fs::write(
            f.env.home.join(".claude.json"),
            serde_json::json!({ "mcpServers": { "cua-computer-use": { "command": SPACES_DRIVER } } })
                .to_string(),
        )
        .unwrap();
        let r = connect_agents(&f.env, &[Agent::Claude], ours, &no_tools(), false, false);
        assert!(
            by(&r, Agent::Claude).line().ends_with(
                "remove its `cua-computer-use` server and run `cua-driver connect` again."
            ),
            "{}",
            by(&r, Agent::Claude).line()
        );
        assert!(!calls(&f).contains("add"), "{}", calls(&f));
    }

    #[cfg(unix)]
    #[test]
    fn a_server_that_reaches_this_driver_through_a_link_is_already_connected() {
        let f = fixture();
        fake_cli(&f, "claude", "exit 0");
        fake_cli(&f, "codex", "exit 0");
        let ours = f.bin_dir.join("cua-driver-0.1");
        std::fs::write(&ours, "").unwrap();
        let link = f.bin_dir.join("cua-driver");
        std::os::unix::fs::symlink(&ours, &link).unwrap();
        std::fs::write(
            f.env.home.join(".claude.json"),
            serde_json::json!({ "mcpServers": { "cua-driver": { "command": link } } }).to_string(),
        )
        .unwrap();
        std::fs::create_dir_all(&f.env.codex_home).unwrap();
        std::fs::write(
            f.env.codex_home.join("config.toml"),
            format!("[mcp_servers.cua-driver]\ncommand = '{}'\n", link.display()),
        )
        .unwrap();
        let r = connect_agents(
            &f.env,
            &[],
            ours.to_str().unwrap(),
            &no_tools(),
            false,
            false,
        );
        assert_eq!(by(&r, Agent::Claude).status, Status::AlreadyConnected);
        assert_eq!(by(&r, Agent::Codex).status, Status::AlreadyConnected);
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
        let r = connect_agents(
            &f.env,
            &[Agent::Claude],
            "/b/cua-driver",
            &no_tools(),
            false,
            false,
        );
        assert_eq!(by(&r, Agent::Claude).status, Status::AlreadyConnected);
        std::fs::create_dir_all(f.env.home.join(".cursor")).unwrap();
        let first = connect_agents(
            &f.env,
            &[Agent::Cursor],
            "/b/cua-driver",
            &no_tools(),
            false,
            false,
        );
        assert_eq!(by(&first, Agent::Cursor).status, Status::Connected);
        let second = connect_agents(
            &f.env,
            &[Agent::Cursor],
            "/b/cua-driver",
            &no_tools(),
            false,
            false,
        );
        assert_eq!(by(&second, Agent::Cursor).status, Status::AlreadyConnected);
    }

    #[test]
    fn cursor_merge_keeps_other_servers_and_keys() {
        let existing = r#"{"theme":"x","mcpServers":{"other":{"command":"o","args":[]}}}"#;
        let merged = merge_mcp_json(Some(existing), "/b/cua-driver")
            .unwrap()
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["theme"], "x");
        assert_eq!(v["mcpServers"]["other"]["command"], "o");
        assert_eq!(v["mcpServers"]["cua-driver"]["command"], "/b/cua-driver");
        assert_eq!(v["mcpServers"]["cua-driver"]["args"][0], "mcp");
        assert_eq!(
            merge_mcp_json(Some(&merged), "/b/cua-driver").unwrap(),
            None
        );
        assert!(merge_mcp_json(None, "/b")
            .unwrap()
            .unwrap()
            .contains("cua-driver"));
    }

    fn droid_tools() -> serde_json::Value {
        serde_json::json!({ "tools": [
            { "name": "list_windows", "annotations": { "readOnlyHint": true } },
            { "name": "click", "annotations": { "readOnlyHint": false, "destructiveHint": true } },
            { "name": "start_session", "annotations": { "readOnlyHint": false } },
            { "name": "kill_app", "annotations": { "readOnlyHint": false, "destructiveHint": true } },
        ]})
    }

    #[test]
    fn droid_gets_the_server_and_medium_ratings_for_action_tools() {
        let f = fixture();
        let dir = f.env.home.join(".factory");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("settings.json"),
            r#"{"logoAnimation":"off","mcpAutonomyOverrides":{"cua-driver":{"tools":{"click":"high"}}}}"#,
        )
        .unwrap();
        let r = connect_agents(
            &f.env,
            &[Agent::Droid],
            "/b/cua-driver",
            &droid_tools(),
            false,
            false,
        );
        assert_eq!(by(&r, Agent::Droid).line(), "Factory Droid: connected");

        let mcp: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("mcp.json")).unwrap()).unwrap();
        assert_eq!(mcp["mcpServers"]["cua-driver"]["command"], "/b/cua-driver");
        assert_eq!(mcp["mcpServers"]["cua-driver"]["args"][0], "mcp");
        assert_eq!(mcp["mcpServers"]["cua-driver"]["type"], "stdio");

        let settings: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap())
                .unwrap();
        let rated = &settings["mcpAutonomyOverrides"]["cua-driver"]["tools"];
        assert_eq!(settings["logoAnimation"], "off");
        assert_eq!(rated["click"], "high", "a rating the user chose is kept");
        assert_eq!(rated["start_session"], "medium");
        assert!(
            rated.get("list_windows").is_none(),
            "read-only tools are already low"
        );
        assert!(
            rated.get("kill_app").is_none(),
            "kill_app keeps Droid's default"
        );

        let again = connect_agents(
            &f.env,
            &[Agent::Droid],
            "/b/cua-driver",
            &droid_tools(),
            false,
            false,
        );
        assert_eq!(by(&again, Agent::Droid).status, Status::AlreadyConnected);
    }

    #[test]
    fn droid_invalid_settings_are_never_overwritten() {
        let f = fixture();
        let dir = f.env.home.join(".factory");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), "{ not json").unwrap();
        let r = connect_agents(&f.env, &[Agent::Droid], "/b", &droid_tools(), false, false);
        assert_eq!(by(&r, Agent::Droid).status, Status::Failed);
        assert_eq!(
            std::fs::read_to_string(dir.join("settings.json")).unwrap(),
            "{ not json"
        );
        assert!(!dir.join("mcp.json").exists(), "nothing is half-written");
    }

    #[test]
    fn cursor_invalid_json_is_never_overwritten() {
        let f = fixture();
        let dir = f.env.home.join(".cursor");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("mcp.json"), "{ not json").unwrap();
        let r = connect_agents(&f.env, &[Agent::Cursor], "/b", &no_tools(), false, false);
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
        std::fs::create_dir_all(f.env.home.join(".factory")).unwrap();
        let r = connect_agents(&f.env, &[], "/b/cua-driver", &droid_tools(), true, false);
        assert!(r.iter().all(|r| r.status == Status::WouldConnect));
        assert_eq!(calls(&f), "");
        assert!(!f.env.home.join(".cursor/mcp.json").exists());
        assert!(!f.env.home.join(".factory/mcp.json").exists());
        assert!(!f.env.home.join(".factory/settings.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn one_failure_does_not_fail_the_others_or_the_exit_code() {
        let f = fixture();
        fake_cli(&f, "claude", "echo boom >&2; exit 1");
        fake_cli(&f, "codex", "exit 0");
        let r = connect_agents(&f.env, &[], "/b/cua-driver", &no_tools(), false, false);
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
        let r = connect_agents(&f.env, &[], "/b/cua-driver", &no_tools(), false, false);
        assert!(exit_failed(&r));
    }

    #[cfg(unix)]
    #[test]
    fn client_filter_limits_the_agents() {
        let f = fixture();
        fake_cli(&f, "claude", "exit 0");
        fake_cli(&f, "codex", "exit 0");
        let r = connect_agents(
            &f.env,
            &[Agent::Codex],
            "/b/cua-driver",
            &no_tools(),
            false,
            false,
        );
        assert_eq!(r.len(), 1);
        assert!(!calls(&f).contains("claude"));
    }
}
