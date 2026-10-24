//! Agent onboarding for cua: detect installed AI coding agents, install the
//! bundled cua skills into their skills directories, and configure the cua
//! MCP server (`cua mcp`) in their MCP configs.
//!
//! - Data-driven: [`registry::AGENTS`] lists each agent's detection hints,
//!   user-scope skills directory and MCP config (path, format, key path,
//!   entry shape), taken from the agent's official docs.
//! - Safe edits: configs are parsed and edited structurally (JSONC through a
//!   concrete syntax tree, TOML through toml_edit, YAML by a verified line
//!   splice), so comments, formatting and every other server are kept. A malformed file is reported and left
//!   untouched. The first write to a file takes a timestamped backup; writes
//!   are atomic and keep permissions and symlinks.
//! - Reversible: `~/.cua/agent-setup.json` records exactly what cua wrote,
//!   and [`AgentSetup::remove`] undoes only that.
//! - Host-safe: everything resolves from a [`HostEnv`] (HOME, XDG and agent
//!   overrides, PATH, app folders), so tests run against a temporary home.

pub mod config;
mod edit;
mod fsutil;
pub mod registry;
pub mod skills;
mod state;

pub use cua_home::channel::CuaChannel;
pub use edit::Format;
pub use fsutil::hash_dir;
pub use registry::{AGENTS, AgentSpec};
pub use skills::Skill;

/// Reads entry `name` under `key_path` of config `text` (tooling, tests).
#[doc(hidden)]
pub fn edit_get(
    format: Format,
    file: &Path,
    text: &str,
    key_path: &[&str],
    name: &str,
) -> Result<Option<Value>> {
    edit::get(format, file, text, key_path, name)
}

/// Ordered entry fields as one JSON object (tooling, tests).
#[doc(hidden)]
pub fn edit_value(fields: &[(String, Value)]) -> Value {
    Value::Object(fields.iter().cloned().collect())
}

use registry::McpSpec;
use serde::Serialize;
use serde_json::Value;
use state::{McpRecord, SkillRecord, State};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// Errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// I/O failure on a path.
    #[error("{path}: {source}")]
    Io {
        /// Path.
        path: PathBuf,
        /// Cause.
        source: std::io::Error,
    },
    /// A config file could not be parsed or has an unexpected shape; it was
    /// left untouched.
    #[error("{path} was left unchanged: {detail}")]
    Malformed {
        /// Path.
        path: PathBuf,
        /// What is wrong.
        detail: String,
    },
    /// Bad input.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// Bug.
    #[error("internal: {0}")]
    Internal(String),
}

impl Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Error::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Host OS family (path conventions).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    /// macOS.
    MacOs,
    /// Windows.
    Windows,
    /// Linux and other Unix.
    Linux,
}

impl Os {
    /// This build's OS.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Os::MacOs
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

/// Everything detection and path resolution read from the host.
#[derive(Clone, Debug)]
pub struct HostEnv {
    /// Home directory.
    pub home: PathBuf,
    /// Environment variables consulted (`XDG_CONFIG_HOME`, `APPDATA`,
    /// `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `CUA_HOME`, ...).
    pub vars: BTreeMap<String, String>,
    /// Directories searched for agent binaries.
    pub path: Vec<PathBuf>,
    /// Folders searched for macOS app bundles.
    pub app_dirs: Vec<PathBuf>,
    /// OS family.
    pub os: Os,
    /// Whether an agent's own CLI may be run to write its config
    /// ([`registry::OwnerCli`]). Off for isolated environments.
    pub run_agent_clis: bool,
    /// Which Cua Spaces build this runs for. It names what is written into
    /// the agents' configs (the MCP server `cua` or `cua-local`, the skills
    /// `cua-spaces` or `cua-spaces-local`) and the folder of the state
    /// (`~/.cua` or `~/.cua-local`), so Local never replaces Stable's.
    pub channel: CuaChannel,
}

const VARS: &[&str] = &[
    "XDG_CONFIG_HOME",
    "APPDATA",
    "LOCALAPPDATA",
    "CUA_HOME",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "GEMINI_CLI_HOME",
    "CLINE_DATA_DIR",
    "OPENCLAW_STATE_DIR",
    "OPENCODE_CONFIG_DIR",
    "PI_CODING_AGENT_DIR",
    "COPILOT_HOME",
    "HERMES_HOME",
];

impl HostEnv {
    /// Ordered fallback binary directories for this environment's OS, searched after PATH.
    pub fn well_known_bin_dirs(&self) -> Vec<PathBuf> {
        if self.os == Os::Windows {
            let appdata = self
                .var("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| self.home.join("AppData/Roaming"));
            vec![
                self.home.join(".local/bin"),
                appdata.join("npm"),
                self.home.join(".bun/bin"),
                self.home.join(".cargo/bin"),
            ]
        } else {
            vec![
                self.home.join(".local/bin"),
                self.home.join(".claude/local"),
                self.home.join(".npm-global/bin"),
                self.home.join(".bun/bin"),
                self.home.join(".cargo/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
            ]
        }
    }

    /// The real environment: `$HOME` (or `%USERPROFILE%`), `$PATH`, the
    /// agent override variables, `/Applications` and `~/Applications`
    /// (or `$CUA_AGENT_APP_DIRS`).
    pub fn current() -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let vars = VARS
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
            .collect();
        let os = Os::current();
        let path: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        // `CUA_AGENT_APP_DIRS` (a path list, empty for none) replaces the
        // app folders, so hermetic runs do not see the host's apps.
        let app_dirs = match std::env::var_os("CUA_AGENT_APP_DIRS") {
            Some(v) => std::env::split_paths(&v)
                .filter(|p| !p.as_os_str().is_empty())
                .collect(),
            None if os == Os::MacOs => {
                vec![PathBuf::from("/Applications"), home.join("Applications")]
            }
            None => vec![],
        };
        let mut env = HostEnv {
            home,
            vars,
            path,
            app_dirs,
            os,
            run_agent_clis: true,
            channel: CuaChannel::current(),
        };
        // A GUI app (Cua Spaces from the Dock) runs without the user's
        // shell PATH, so an agent installed under a user bin dir would look
        // missing. Search the usual install dirs after the real PATH.
        for dir in env.well_known_bin_dirs() {
            if !env.path.contains(&dir) {
                env.path.push(dir);
            }
        }
        env
    }

    /// A hermetic environment rooted at `home`: no variables, no PATH, apps
    /// only under `home/Applications`, no agent CLIs run. Tests add what
    /// they need.
    pub fn isolated(home: impl Into<PathBuf>) -> Self {
        let home = home.into();
        HostEnv {
            app_dirs: vec![home.join("Applications")],
            home,
            vars: BTreeMap::new(),
            path: vec![],
            os: Os::current(),
            run_agent_clis: false,
            channel: CuaChannel::Stable,
        }
    }

    /// This environment for `channel` (a Local app's, in tests).
    pub fn with_channel(mut self, channel: CuaChannel) -> Self {
        self.channel = channel;
        self
    }

    /// A non-empty variable.
    pub fn var(&self, k: &str) -> Option<&str> {
        self.vars
            .get(k)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// `bin` on [`HostEnv::path`].
    pub fn which(&self, bin: &str) -> Option<PathBuf> {
        let exts: &[&str] = if self.os == Os::Windows {
            &[".exe", ".cmd", ".bat", ""]
        } else {
            &[""]
        };
        for dir in &self.path {
            for ext in exts {
                let p = dir.join(format!("{bin}{ext}"));
                if is_executable(&p) {
                    return Some(p);
                }
            }
        }
        None
    }

    /// `$CUA_HOME`, else the channel's home: `~/.cua`, or `~/.cua-local` for
    /// Local ([`CuaChannel::home_dir_name`]).
    pub fn cua_home(&self) -> PathBuf {
        self.var("CUA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join(self.channel.home_dir_name()))
    }
}

fn is_executable(p: &Path) -> bool {
    let Ok(m) = std::fs::metadata(p) else {
        return false;
    };
    if !m.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        m.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    true
}

/// The MCP server cua registers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct McpServer {
    /// Server name in the agent's config (`cua`).
    pub name: String,
    /// Launch command (an absolute `cua` path when known).
    pub command: String,
    /// Arguments (`["mcp"]`).
    pub args: Vec<String>,
    /// Extra environment.
    pub env: BTreeMap<String, String>,
}

/// The server name cua registers under.
pub const MCP_SERVER_NAME: &str = "cua";

/// The server name cua-driver (background computer-use) registers under.
pub const DRIVER_MCP_SERVER_NAME: &str = "cua-driver";

/// The bundled skill that teaches agents cua-driver.
pub const DRIVER_SKILL: &str = "cua-driver";

/// The name the retired Spaces Python MCP server (`spaces_mcp.py`, replaced
/// by `cua daemon mcp`) was registered under by older Spaces app builds.
pub const LEGACY_SPACES_MCP_NAME: &str = "cua-spaces";

impl McpServer {
    /// `command mcp` under the name `cua`.
    pub fn new(command: impl Into<String>) -> Self {
        McpServer {
            name: MCP_SERVER_NAME.into(),
            command: command.into(),
            args: vec!["mcp".into()],
            env: BTreeMap::new(),
        }
    }

    /// Adds an environment variable.
    pub fn with_env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.insert(k.into(), v.into());
        self
    }

    /// `command mcp` under the name the channel registers: `cua`, or
    /// `cua-local` for Local ([`CuaChannel::mcp_server_name`]).
    pub fn for_channel(channel: CuaChannel, command: impl Into<String>) -> Self {
        McpServer {
            name: channel.mcp_server_name().into(),
            ..McpServer::new(command)
        }
    }

    /// `cua mcp` (`cua-local mcp` for Local), with the channel's CLI
    /// resolved to an absolute path on `env`'s PATH (GUI agents often run
    /// without the user's shell PATH), else its bare name.
    pub fn default_for(env: &HostEnv) -> Self {
        let cli = env.channel.cli_name();
        let cmd = env
            .which(cli)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| cli.into());
        McpServer::for_channel(env.channel, cmd)
    }
}

impl McpServer {
    /// `command mcp` under the name `cua-driver`: the cua-driver MCP server
    /// agents use for background computer-use.
    pub fn driver(command: impl Into<String>) -> Self {
        McpServer {
            name: DRIVER_MCP_SERVER_NAME.into(),
            ..McpServer::new(command)
        }
    }

    /// `cua-driver mcp`, with `cua-driver` resolved to an absolute path on
    /// `env`'s PATH, else in `~/.local/bin` (where cua-driver's installer
    /// puts it; GUI apps often run without the user's shell PATH), else the
    /// bare name.
    pub fn driver_for(env: &HostEnv) -> Self {
        let bin = if env.os == Os::Windows {
            "cua-driver.exe"
        } else {
            "cua-driver"
        };
        let cmd = env
            .which("cua-driver")
            .or_else(|| {
                Some(env.home.join(".local").join("bin").join(bin)).filter(|p| is_executable(p))
            })
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "cua-driver".into());
        McpServer::driver(cmd)
    }
}

/// Whether a launch command is the channel's cua binary: for Stable `cua`,
/// `/x/bin/cua`, `cua.exe`; for Local `cua-local` (`cua-local.exe`), or the
/// `cua` inside `Cua Spaces Local.app`. A Stable `cua` is not Local's, and
/// the reverse.
fn is_cua_command(cmd: &str, channel: CuaChannel) -> bool {
    let base = cmd.rsplit(['/', '\\']).next().unwrap_or(cmd);
    match channel {
        CuaChannel::Stable => base == "cua" || base == "cua.exe",
        CuaChannel::Local => {
            base == "cua-local"
                || base == "cua-local.exe"
                || CuaChannel::path_is_local(Path::new(cmd))
        }
    }
}

/// Detection result for one agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AgentStatus {
    /// Registry id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Whether the agent looks installed.
    pub installed: bool,
    /// What gave it away (`bin:/usr/local/bin/claude`, `dir:~/.codex`, ...).
    pub evidence: Vec<String>,
    /// User skills directory, when supported.
    pub skills_dir: Option<PathBuf>,
    /// User MCP config file, when supported.
    pub mcp_config: Option<PathBuf>,
    /// Config syntax.
    pub mcp_format: Option<Format>,
    /// A `cua` server launching a cua binary is configured.
    pub cua_configured: bool,
    /// That entry is the one cua wrote (recorded in agent-setup.json).
    pub cua_managed: bool,
    /// A `cua-driver` server (background computer-use) is configured.
    pub cua_driver_configured: bool,
    /// Bundled skills present in the skills directory.
    pub skills_installed: Vec<String>,
    /// Installed bundled skills whose content differs from this build's.
    pub skills_outdated: Vec<String>,
    /// A problem reading the MCP config (malformed file, ...).
    pub error: Option<String>,
    /// What could not be verified from official docs.
    pub unverified: Option<String>,
    /// The account the agent signs in with on its own ("ChatGPT"), when
    /// cua can see that sign-in ([`registry::LoginSpec`]).
    pub sign_in: Option<String>,
    /// That sign-in is on this computer (its file is there and not empty).
    pub signed_in: bool,
}

/// What happened to one target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Change {
    /// Written where nothing was.
    Created,
    /// Replaced or edited.
    Updated,
    /// Already as wanted.
    Unchanged,
    /// Removed (or the previous entry restored).
    Removed,
    /// Deliberately left alone (see detail).
    Skipped,
    /// Error (see detail); the target was left untouched.
    Failed,
}

/// What an operation did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// A skill folder.
    Skill,
    /// An MCP config entry.
    Mcp,
}

/// One target's outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Outcome {
    /// Agents served by this target.
    pub agents: Vec<String>,
    /// Kind.
    pub target: Target,
    /// Skill name or MCP server name.
    pub item: String,
    /// Skill folder or config file.
    pub path: PathBuf,
    /// Result.
    pub change: Change,
    /// Explanation.
    pub detail: String,
    /// Backup taken before this write.
    pub backup: Option<PathBuf>,
}

impl Outcome {
    /// Whether this is a failure.
    pub fn failed(&self) -> bool {
        self.change == Change::Failed
    }
}

/// Which parts [`AgentSetup::remove`] undoes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Parts {
    /// Skill folders.
    pub skills: bool,
    /// MCP entries.
    pub mcp: bool,
}

impl Parts {
    /// Both.
    pub const ALL: Parts = Parts {
        skills: true,
        mcp: true,
    };
}

/// The onboarding engine.
#[derive(Clone, Debug)]
pub struct AgentSetup {
    env: HostEnv,
    state_path: PathBuf,
}

impl AgentSetup {
    /// On `env`; state in `<cua home>/agent-setup.json`.
    pub fn new(env: HostEnv) -> Self {
        let state_path = env.cua_home().join("agent-setup.json");
        AgentSetup { env, state_path }
    }

    /// On the real environment.
    pub fn from_env() -> Self {
        Self::new(HostEnv::current())
    }

    /// The environment.
    pub fn env(&self) -> &HostEnv {
        &self.env
    }

    /// The state file.
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// The bundled default skills, as this channel installs them (Local's
    /// carry `-local` names: [`CuaChannel::skill_install_name`]).
    pub fn bundled_skills(&self) -> Vec<Skill> {
        skills::bundled_for(self.env.channel)
    }

    /// Resolves an agent selection: `all` (installed agents), `none`, or a
    /// comma-separated list of ids/aliases. Unknown ids are an error.
    pub fn select(&self, selection: &str) -> Result<Vec<String>> {
        let s = selection.trim();
        match s {
            "" | "none" => Ok(vec![]),
            "all" => Ok(self
                .detect()
                .into_iter()
                .filter(|a| a.installed)
                .map(|a| a.id)
                .collect()),
            _ => {
                let mut out = Vec::new();
                for id in s.split(',').map(str::trim).filter(|x| !x.is_empty()) {
                    let a = registry::find(id).ok_or_else(|| {
                        Error::InvalidArgument(format!(
                            "unknown agent {id:?}; known: {}",
                            AGENTS.iter().map(|a| a.id).collect::<Vec<_>>().join(", ")
                        ))
                    })?;
                    if !out.contains(&a.id.to_string()) {
                        out.push(a.id.to_string());
                    }
                }
                Ok(out)
            }
        }
    }

    fn specs(&self, agents: &[String]) -> Result<Vec<&'static AgentSpec>> {
        agents
            .iter()
            .map(|id| {
                registry::find(id)
                    .ok_or_else(|| Error::InvalidArgument(format!("unknown agent {id:?}")))
            })
            .collect()
    }

    fn mcp_file(&self, m: &McpSpec) -> PathBuf {
        m.alt_files
            .iter()
            .map(|l| l.resolve(&self.env))
            .find(|p| p.exists())
            .unwrap_or_else(|| m.file.resolve(&self.env))
    }

    /// Detects every registry agent (read-only).
    pub fn detect(&self) -> Vec<AgentStatus> {
        let state = State::load(&self.state_path).unwrap_or_default();
        let bundled = self.bundled_skills();
        AGENTS
            .iter()
            .map(|a| self.detect_one(a, &state, &bundled))
            .collect()
    }

    /// Starts `agent`'s own sign-in on this computer (`codex login`: the
    /// browser opens, and the agent keeps the sign-in in its own file). It
    /// returns once the command is running; [`AgentSetup::detect`] reports
    /// `signed_in` when it is done. cua never reads or copies the sign-in,
    /// so every computer signs in on its own. A sign-in already waiting for
    /// the browser is not started twice.
    pub fn sign_in(&self, agent: &str) -> Result<()> {
        let a = registry::find(agent)
            .ok_or_else(|| Error::InvalidArgument(format!("unknown agent {agent:?}")))?;
        let login = a.login.ok_or_else(|| {
            Error::InvalidArgument(format!("{} has no sign-in cua can start", a.name))
        })?;
        if !self.env.run_agent_clis {
            return Err(Error::InvalidArgument(
                "agent commands are off in this environment".into(),
            ));
        }
        let bin = a
            .bins
            .iter()
            .find_map(|b| self.env.which(b))
            .ok_or_else(|| {
                Error::InvalidArgument(format!("{} isn't installed on this computer", a.name))
            })?;
        static WAITING: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
        let mut waiting = WAITING.lock().unwrap_or_else(|e| e.into_inner());
        if waiting.contains(&a.id) {
            return Ok(());
        }
        let program = cmd_shim_target(&bin).unwrap_or_else(|| bin.clone());
        let mut cmd = std::process::Command::new(&program);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        // A GUI app has no shell PATH: an npm-installed CLI needs `node`
        // from the same dirs the agent was found in.
        if let Ok(path) = std::env::join_paths(&self.env.path) {
            cmd.env("PATH", path);
        }
        let mut child = cmd
            .args(login.args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| Error::io(&bin, e))?;
        waiting.push(a.id);
        let id = a.id;
        // Reaped in the background: the sign-in waits for the browser.
        std::thread::spawn(move || {
            let _ = child.wait();
            WAITING
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|w| *w != id);
        });
        Ok(())
    }

    /// Alias of [`AgentSetup::detect`].
    pub fn status(&self) -> Vec<AgentStatus> {
        self.detect()
    }

    fn detect_one(&self, a: &AgentSpec, state: &State, bundled: &[Skill]) -> AgentStatus {
        let mut evidence = Vec::new();
        for b in a.bins {
            if let Some(p) = self.env.which(b) {
                evidence.push(format!("bin:{}", p.display()));
            }
        }
        for m in a.markers {
            let p = m.resolve(&self.env);
            if p.exists() {
                evidence.push(format!("path:{}", p.display()));
            }
        }
        if self.env.os == Os::MacOs {
            for app in a.apps {
                for d in &self.env.app_dirs {
                    let p = d.join(app);
                    if p.is_dir() {
                        evidence.push(format!("app:{}", p.display()));
                    }
                }
            }
        }
        let skills_dir = a.skills.map(|l| l.resolve(&self.env));
        let (mut skills_installed, mut skills_outdated) = (vec![], vec![]);
        if let Some(d) = &skills_dir {
            // Its own directory first, then the ones it also loads.
            let dirs: Vec<PathBuf> = std::iter::once(d.clone())
                .chain(a.also_reads.iter().map(|l| l.resolve(&self.env)))
                .collect();
            for s in bundled {
                if let Some(h) = dirs.iter().find_map(|d| fsutil::hash_dir(&d.join(&s.name))) {
                    skills_installed.push(s.name.clone());
                    if h != s.hash {
                        skills_outdated.push(s.name.clone());
                    }
                }
            }
        }
        let mut status = AgentStatus {
            id: a.id.into(),
            name: a.name.into(),
            installed: !evidence.is_empty(),
            evidence,
            skills_dir,
            mcp_config: None,
            mcp_format: None,
            cua_configured: false,
            cua_managed: false,
            cua_driver_configured: false,
            skills_installed,
            skills_outdated,
            error: None,
            unverified: (!a.unverified.is_empty()).then(|| a.unverified.to_string()),
            sign_in: a.login.map(|l| l.label.to_string()),
            signed_in: a.login.is_some_and(|l| {
                std::fs::metadata(l.file.resolve(&self.env))
                    .is_ok_and(|m| m.is_file() && m.len() > 0)
            }),
        };
        if let Some(m) = &a.mcp {
            let file = self.mcp_file(m);
            status.mcp_format = Some(m.format);
            // The channel's own entry (`cua`, or `cua-local`): Local never
            // reads Stable's as its own.
            let name = self.env.channel.mcp_server_name();
            match read_text(&file).and_then(|t| edit::get(m.format, &file, &t, m.key_path, name)) {
                Ok(Some(v)) => {
                    status.cua_configured = m
                        .shape
                        .command_of(&v)
                        .is_some_and(|(c, _)| is_cua_command(&c, self.env.channel));
                    status.cua_managed = state.record(name, &file).is_some_and(|r| r.value == v);
                }
                Ok(None) => {}
                Err(e) => status.error = Some(e.to_string()),
            }
            status.cua_driver_configured = read_text(&file)
                .and_then(|t| edit::get(m.format, &file, &t, m.key_path, DRIVER_MCP_SERVER_NAME))
                .is_ok_and(|v| v.is_some());
            status.mcp_config = Some(file);
        }
        status
    }

    /// Installs `skills` (bundled names; empty = all) into the skills
    /// directories of `agents`. Agents sharing a directory get one copy.
    /// A folder cua did not write, or one edited since, is skipped unless
    /// `force` (then the old folder is kept as a timestamped backup).
    pub fn install_skills(
        &self,
        agents: &[String],
        skill_names: &[String],
        force: bool,
    ) -> Result<Vec<Outcome>> {
        let specs = self.specs(agents)?;
        let bundled = self.bundled_skills();
        let wanted: Vec<&Skill> = if skill_names.is_empty() {
            bundled.iter().collect()
        } else {
            skill_names
                .iter()
                .map(|n| {
                    // By the bundled name (`cua-spaces`) or the name this
                    // channel installs it under (`cua-spaces-local`).
                    bundled
                        .iter()
                        .find(|s| &s.name == n || &s.source == n)
                        .ok_or_else(|| Error::InvalidArgument(format!("unknown skill {n:?}")))
                })
                .collect::<Result<_>>()?
        };
        let mut state = State::load(&self.state_path)?;
        // Skills dir → agents. An agent that also loads another agent's
        // skills directory (Cursor, OpenCode and VS Code read
        // ~/.claude/skills) is served from there when that directory gets,
        // or already holds, cua's copies, so it does not list them twice.
        let primaries: BTreeSet<PathBuf> = specs
            .iter()
            .filter_map(|a| a.skills.map(|l| l.resolve(&self.env)))
            .collect();
        let holds_all = |dir: &Path| {
            wanted.iter().all(|s| {
                let dest = dir.join(&s.name);
                state.skills.get(&dest).is_some_and(|r| r.skill == s.name) && dest.is_dir()
            })
        };
        let mut dirs: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
        let mut out = Vec::new();
        for a in &specs {
            match a.skills {
                Some(l) => {
                    let own = l.resolve(&self.env);
                    let served_by = a
                        .also_reads
                        .iter()
                        .map(|l| l.resolve(&self.env))
                        .find(|d| *d != own && (primaries.contains(d) || holds_all(d)));
                    dirs.entry(served_by.unwrap_or(own))
                        .or_default()
                        .push(a.id.into())
                }
                None => out.push(Outcome {
                    agents: vec![a.id.into()],
                    target: Target::Skill,
                    item: String::new(),
                    path: PathBuf::new(),
                    change: Change::Skipped,
                    detail: format!("{} does not support Agent Skills", a.name),
                    backup: None,
                }),
            }
        }
        for (dir, ids) in dirs {
            for s in &wanted {
                let dest = dir.join(&s.name);
                out.push(self.install_one(&mut state, s, &dest, &ids, force));
            }
        }
        state.save(&self.state_path)?;
        Ok(out)
    }

    fn install_one(
        &self,
        state: &mut State,
        s: &Skill,
        dest: &Path,
        ids: &[String],
        force: bool,
    ) -> Outcome {
        let mut o = Outcome {
            agents: ids.to_vec(),
            target: Target::Skill,
            item: s.name.clone(),
            path: dest.to_path_buf(),
            change: Change::Unchanged,
            detail: format!("{} {}", s.name, s.version),
            backup: None,
        };
        let current = fsutil::hash_dir(dest);
        let record = state.skills.get(dest).cloned();
        let owned_unmodified = match (&record, &current) {
            (Some(r), Some(h)) => &r.hash == h,
            _ => false,
        };
        if current.as_deref() == Some(s.hash.as_str()) {
            o.change = Change::Unchanged;
        } else if current.is_some() && !owned_unmodified && !force {
            o.change = Change::Skipped;
            o.detail = if record.is_some() {
                format!(
                    "{} was edited since cua installed it; use --force to replace",
                    dest.display()
                )
            } else {
                format!(
                    "{} exists and was not installed by cua; use --force to replace",
                    dest.display()
                )
            };
            return o;
        } else {
            if current.is_some() && !owned_unmodified {
                // Forced over foreign or edited content: keep it.
                let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
                let bak = dest.with_file_name(format!("{}.cua-backup-{stamp}", s.name));
                if let Err(e) = std::fs::rename(dest, &bak) {
                    o.change = Change::Failed;
                    o.detail = format!("could not back up {}: {e}", dest.display());
                    return o;
                }
                o.backup = Some(bak);
            }
            if let Err(e) = skills::write_for(self.env.channel, &s.source, dest) {
                o.change = Change::Failed;
                o.detail = e.to_string();
                return o;
            }
            o.change = if current.is_some() {
                Change::Updated
            } else {
                Change::Created
            };
        }
        let mut agents: BTreeSet<String> = record.map(|r| r.agents).unwrap_or_default();
        agents.extend(ids.iter().cloned());
        state.skills.insert(
            dest.to_path_buf(),
            SkillRecord {
                skill: s.name.clone(),
                version: s.version.clone(),
                hash: s.hash.clone(),
                agents,
                installed_at: state::now(),
            },
        );
        o
    }

    /// Configures `server` in the MCP config of each of `agents`.
    pub fn configure_mcp(&self, agents: &[String], server: &McpServer) -> Result<Vec<Outcome>> {
        if server.name.trim().is_empty() || server.command.trim().is_empty() {
            return Err(Error::InvalidArgument(
                "the MCP server needs a name and a command".into(),
            ));
        }
        let specs = self.specs(agents)?;
        let mut state = State::load(&self.state_path)?;
        let mut out = Vec::new();
        for a in specs {
            let Some(m) = &a.mcp else {
                out.push(Outcome {
                    agents: vec![a.id.into()],
                    target: Target::Mcp,
                    item: server.name.clone(),
                    path: PathBuf::new(),
                    change: Change::Skipped,
                    detail: format!("{} does not support MCP servers", a.name),
                    backup: None,
                });
                continue;
            };
            let file = self.mcp_file(m);
            let mut o = Outcome {
                agents: vec![a.id.into()],
                target: Target::Mcp,
                item: server.name.clone(),
                path: file.clone(),
                change: Change::Unchanged,
                detail: String::new(),
                backup: None,
            };
            if let Err(e) = self.configure_one(&mut state, a, m, &file, server, &mut o) {
                o.change = Change::Failed;
                o.detail = e.to_string();
            }
            out.push(o);
            if server.name != LEGACY_SPACES_MCP_NAME {
                out.extend(self.drop_legacy_spaces_mcp(&mut state, a, m, &file));
            }
        }
        state.save(&self.state_path)?;
        Ok(out)
    }

    /// Removes a stale `cua-spaces` entry that still launches the retired
    /// Python `spaces_mcp.py` (its tools now come from the cua server). Any
    /// other `cua-spaces` entry is left alone.
    fn drop_legacy_spaces_mcp(
        &self,
        state: &mut State,
        a: &AgentSpec,
        m: &McpSpec,
        file: &Path,
    ) -> Option<Outcome> {
        let text = read_text(file).ok()?;
        let entry = edit::get(m.format, file, &text, m.key_path, LEGACY_SPACES_MCP_NAME)
            .ok()
            .flatten()?;
        if !entry.to_string().contains("spaces_mcp.py") {
            return None;
        }
        let mut o = Outcome {
            agents: vec![a.id.into()],
            target: Target::Mcp,
            item: LEGACY_SPACES_MCP_NAME.into(),
            path: file.to_path_buf(),
            change: Change::Removed,
            detail: "retired Spaces Python MCP (spaces_mcp.py)".into(),
            backup: None,
        };
        let result = (|| -> Result<()> {
            if !state.backups.contains_key(file)
                && let Some(b) = fsutil::backup(file)?
            {
                state.backups.insert(file.to_path_buf(), b.clone());
                o.backup = Some(b);
            }
            let via_cli = m
                .owner_cli
                .filter(|_| self.env.run_agent_clis)
                .and_then(|c| self.env.which(c.bin()).map(|bin| (c, bin)))
                .map(|(c, bin)| (c, cmd_shim_target(&bin).unwrap_or(bin)));
            if let Some((cli, bin)) = via_cli {
                return run_cli(&bin, &cli.remove_args(LEGACY_SPACES_MCP_NAME));
            }
            if let Some(new) =
                edit::remove(m.format, file, &text, m.key_path, LEGACY_SPACES_MCP_NAME)?
            {
                fsutil::atomic_write(file, new.as_bytes())?;
            }
            Ok(())
        })();
        if let Err(e) = result {
            o.change = Change::Failed;
            o.detail = e.to_string();
        }
        Some(o)
    }

    fn configure_one(
        &self,
        state: &mut State,
        a: &AgentSpec,
        m: &McpSpec,
        file: &Path,
        server: &McpServer,
        o: &mut Outcome,
    ) -> Result<()> {
        let text = read_text(file)?;
        edit::validate(m.format, file, &text)?;
        let fields = m.shape.fields(server);
        let want = edit::to_value(&fields);
        let existing = edit::get(m.format, file, &text, m.key_path, &server.name)?;
        let record = state.record(&server.name, file).cloned();
        let remember = |state: &mut State, previous: Option<Value>| {
            let mut agents = record
                .as_ref()
                .map(|r| r.agents.clone())
                .unwrap_or_default();
            agents.insert(a.id.to_string());
            state.records_mut(&server.name).insert(
                file.to_path_buf(),
                McpRecord {
                    agents,
                    format: m.format,
                    key_path: m.key_path.iter().map(|s| s.to_string()).collect(),
                    name: server.name.clone(),
                    value: want.clone(),
                    previous,
                    written_at: state::now(),
                },
            );
        };
        // The entry that was there before cua first wrote it (restored on
        // remove), unless it was itself a cua registration.
        let previous = match &record {
            Some(r) => r.previous.clone(),
            None => existing.clone().filter(|v| {
                !m.shape
                    .command_of(v)
                    .is_some_and(|(c, _)| is_cua_command(&c, self.env.channel))
            }),
        };
        if existing.as_ref() == Some(&want) {
            o.change = Change::Unchanged;
            o.detail = "already configured".into();
            remember(state, previous);
            return Ok(());
        }
        let via_cli = m
            .owner_cli
            .filter(|_| self.env.run_agent_clis)
            .and_then(|c| self.env.which(c.bin()).map(|bin| (c, bin)));
        // The edit is computed first, so a file it refuses is not backed up.
        let edited = match via_cli {
            Some(_) => None,
            None => Some(edit::upsert(
                m.format,
                file,
                &text,
                m.key_path,
                &server.name,
                &fields,
            )?),
        };
        if !state.backups.contains_key(file)
            && let Some(b) = fsutil::backup(file)?
        {
            state.backups.insert(file.to_path_buf(), b.clone());
            o.backup = Some(b);
        }
        if let Some((cli, bin)) = via_cli {
            if existing.is_some() {
                run_cli(&bin, &cli.remove_args(&server.name))?;
            }
            run_cli(
                &bin,
                &cli.add_args(&server.name, &edit::fields_json(&fields)),
            )?;
            let back = edit::get(m.format, file, &read_text(file)?, m.key_path, &server.name)?;
            if back.as_ref() != Some(&want) {
                return Err(Error::Internal(format!(
                    "`{} mcp add-json` succeeded but {} does not hold the expected entry",
                    cli.bin(),
                    file.display()
                )));
            }
            o.detail = format!("via `{} mcp add-json`", cli.bin());
        } else if let Some(new) = edited {
            fsutil::atomic_write(file, new.as_bytes())?;
        }
        o.change = if existing.is_some() {
            Change::Updated
        } else {
            Change::Created
        };
        if o.detail.is_empty() {
            o.detail = format!("{} {}", server.command, server.args.join(" "));
        }
        remember(state, previous);
        Ok(())
    }

    /// Undoes what cua wrote for `agents`: MCP entries that still hold the
    /// exact value cua wrote (the entry that was there before, if any, is
    /// restored), and skill folders cua installed and nobody edited (a
    /// folder shared with an agent not being removed stays).
    pub fn remove(&self, agents: &[String], parts: Parts) -> Result<Vec<Outcome>> {
        let specs = self.specs(agents)?;
        let mut state = State::load(&self.state_path)?;
        let mut out = Vec::new();
        if parts.mcp {
            for a in &specs {
                let Some(m) = &a.mcp else { continue };
                let file = self.mcp_file(m);
                // The channel's server (`cua`, or `cua-local`) always gets a
                // row; other servers (`cua-driver`) only when cua registered
                // them there.
                let mut names = state.names_in(&file);
                let own = self.env.channel.mcp_server_name();
                if state.record(own, &file).is_none() {
                    names.insert(0, own.to_string());
                }
                for name in names {
                    let mut o = Outcome {
                        agents: vec![a.id.into()],
                        target: Target::Mcp,
                        item: name.clone(),
                        path: file.clone(),
                        change: Change::Skipped,
                        detail: String::new(),
                        backup: None,
                    };
                    if let Err(e) = self.remove_mcp(&mut state, a, m, &file, &name, &mut o) {
                        o.change = Change::Failed;
                        o.detail = e.to_string();
                    }
                    out.push(o);
                }
            }
        }
        if parts.skills {
            let ids: BTreeSet<String> = specs.iter().map(|a| a.id.to_string()).collect();
            // Their own directories and the ones they are served from.
            let dirs: BTreeSet<PathBuf> = specs
                .iter()
                .flat_map(|a| a.skills.iter().chain(a.also_reads))
                .map(|l| l.resolve(&self.env))
                .collect();
            let owned: Vec<PathBuf> = state
                .skills
                .keys()
                .filter(|p| p.parent().is_some_and(|d| dirs.contains(d)))
                .cloned()
                .collect();
            for dest in owned {
                let mut r = state.skills[&dest].clone();
                let removing: Vec<String> = r.agents.intersection(&ids).cloned().collect();
                if removing.is_empty() {
                    continue;
                }
                r.agents.retain(|x| !ids.contains(x));
                let mut o = Outcome {
                    agents: removing,
                    target: Target::Skill,
                    item: r.skill.clone(),
                    path: dest.clone(),
                    change: Change::Skipped,
                    detail: String::new(),
                    backup: None,
                };
                if !r.agents.is_empty() {
                    o.detail = format!(
                        "kept for {}",
                        r.agents.iter().cloned().collect::<Vec<_>>().join(", ")
                    );
                    state.skills.insert(dest.clone(), r);
                } else {
                    match fsutil::hash_dir(&dest) {
                        None => {
                            o.change = Change::Removed;
                            o.detail = "already gone".into();
                            state.skills.remove(&dest);
                        }
                        Some(h) if h == r.hash => match std::fs::remove_dir_all(&dest) {
                            Ok(()) => {
                                o.change = Change::Removed;
                                state.skills.remove(&dest);
                            }
                            Err(e) => {
                                o.change = Change::Failed;
                                o.detail = e.to_string();
                            }
                        },
                        Some(_) => {
                            o.detail = "edited since cua installed it; left in place".into();
                            state.skills.remove(&dest);
                        }
                    }
                }
                out.push(o);
            }
        }
        state.save(&self.state_path)?;
        Ok(out)
    }

    fn remove_mcp(
        &self,
        state: &mut State,
        a: &AgentSpec,
        m: &McpSpec,
        file: &Path,
        name: &str,
        o: &mut Outcome,
    ) -> Result<()> {
        // `cua`'s record may carry another name (older states).
        let key = if state.mcp.get(file).is_some_and(|r| r.name == name) {
            MCP_SERVER_NAME
        } else {
            name
        };
        let Some(mut r) = state.record(key, file).cloned() else {
            o.detail = "not configured by cua".into();
            return Ok(());
        };
        let text = read_text(file)?;
        let current = edit::get(m.format, file, &text, m.key_path, &r.name)?;
        o.item = r.name.clone();
        if current.as_ref() != Some(&r.value) {
            o.detail = if current.is_none() {
                "already removed".into()
            } else {
                "the entry changed since cua wrote it; left in place".into()
            };
            state.forget(key, file);
            return Ok(());
        }
        let via_cli = m
            .owner_cli
            .filter(|_| self.env.run_agent_clis)
            .and_then(|c| self.env.which(c.bin()).map(|bin| (c, bin)));
        if let (Some((cli, bin)), None) = (via_cli, &r.previous) {
            run_cli(&bin, &cli.remove_args(&r.name))?;
        } else {
            let new = match &r.previous {
                Some(prev) => {
                    let fields: edit::Fields = prev
                        .as_object()
                        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                        .unwrap_or_default();
                    Some(edit::upsert(
                        m.format, file, &text, m.key_path, &r.name, &fields,
                    )?)
                }
                None => edit::remove(m.format, file, &text, m.key_path, &r.name)?,
            };
            if let Some(new) = new {
                fsutil::atomic_write(file, new.as_bytes())?;
            }
        }
        o.change = Change::Removed;
        o.detail = if r.previous.is_some() {
            "restored the previous entry".into()
        } else {
            "removed".into()
        };
        r.agents.remove(a.id);
        state.forget(key, file);
        Ok(())
    }

    /// Background computer-use for `agents`: the bundled `cua-driver` skill
    /// and the `cua-driver` MCP server (`server`, usually
    /// [`McpServer::driver_for`]). `parts` picks either or both. Agents
    /// without MCP support get the skill only. The same step backs
    /// `cua agents setup --cua-driver`, the installers' `cua-driver` item and
    /// the Spaces onboarding card.
    pub fn setup_cua_driver(
        &self,
        agents: &[String],
        server: &McpServer,
        parts: Parts,
        force: bool,
    ) -> Result<Vec<Outcome>> {
        if server.name != DRIVER_MCP_SERVER_NAME {
            return Err(Error::InvalidArgument(format!(
                "the cua-driver server must be named {DRIVER_MCP_SERVER_NAME:?}"
            )));
        }
        let mut out = Vec::new();
        if parts.skills {
            out.extend(self.install_skills(agents, &[DRIVER_SKILL.to_string()], force)?);
        }
        if parts.mcp {
            let with_mcp: Vec<String> = self
                .specs(agents)?
                .iter()
                .filter(|a| a.mcp.is_some())
                .map(|a| a.id.to_string())
                .collect();
            out.extend(self.configure_mcp(&with_mcp, server)?);
        }
        Ok(out)
    }

    /// Refreshes every skill folder cua installed to this build's content
    /// (edited folders are skipped), and re-points every MCP entry cua
    /// manages that still holds its recorded value to `server`.
    pub fn update(&self, server: Option<&McpServer>) -> Result<Vec<Outcome>> {
        let state = State::load(&self.state_path)?;
        let mut out = Vec::new();
        let mut by_dir: BTreeMap<PathBuf, (Vec<String>, Vec<String>)> = BTreeMap::new();
        for (dest, r) in &state.skills {
            if let Some(dir) = dest.parent() {
                let e = by_dir.entry(dir.to_path_buf()).or_default();
                e.0.extend(r.agents.iter().cloned());
                e.1.push(r.skill.clone());
            }
        }
        for (_dir, (agents, names)) in by_dir {
            let mut agents = agents;
            agents.sort();
            agents.dedup();
            let known: Vec<String> = agents
                .into_iter()
                .filter(|a| registry::find(a).is_some())
                .collect();
            let bundled = self.bundled_skills();
            let names: Vec<String> = names
                .into_iter()
                .filter(|n| bundled.iter().any(|s| &s.name == n))
                .collect();
            if known.is_empty() || names.is_empty() {
                continue;
            }
            out.extend(self.install_skills(&known, &names, false)?);
        }
        if let Some(server) = server {
            let agents: Vec<String> = state
                .records(&server.name)
                .into_iter()
                .flat_map(|r| r.values())
                .flat_map(|r| r.agents.iter().cloned())
                .filter(|a| registry::find(a).is_some())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            for a in agents {
                let spec = registry::find(&a).expect("filtered");
                let Some(m) = &spec.mcp else { continue };
                let file = self.mcp_file(m);
                let text = read_text(&file).unwrap_or_default();
                let current = edit::get(m.format, &file, &text, m.key_path, &server.name)
                    .ok()
                    .flatten();
                let recorded = state.record(&server.name, &file).map(|r| r.value.clone());
                if current.is_some() && current == recorded {
                    out.extend(self.configure_mcp(&[a], server)?);
                }
            }
        }
        Ok(out)
    }
}

fn read_text(file: &Path) -> Result<String> {
    match std::fs::read(file) {
        Ok(b) => String::from_utf8(b).map_err(|_| Error::Malformed {
            path: file.to_path_buf(),
            detail: "not UTF-8 text".into(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(Error::io(file, e)),
    }
}

/// The program an npm `.cmd` shim runs, when it is a native executable
/// next to the shim (`"%dp0%\node_modules\...\claude.exe" %*`, as the
/// Claude Code npm package installs).
///
/// Windows runs a `.cmd` through cmd.exe, so Rust escapes each `"` of an
/// argument as `""`; Claude Code's native `claude.exe` does not read that
/// back, and `claude mcp add-json` got mangled JSON ("Invalid configuration:
/// : Invalid input", CUA-1703). Running the executable directly uses the
/// normal `\"` escaping, which it parses.
fn cmd_shim_target(shim: &Path) -> Option<PathBuf> {
    let ext = shim.extension()?.to_str()?.to_ascii_lowercase();
    if ext != "cmd" && ext != "bat" {
        return None;
    }
    let text = std::fs::read_to_string(shim).ok()?;
    let dir = shim.parent()?;
    text.lines().find_map(|line| {
        let rest = line.trim().strip_prefix('"')?;
        let rest = rest
            .strip_prefix("%dp0%\\")
            .or_else(|| rest.strip_prefix("%~dp0\\"))
            .or_else(|| rest.strip_prefix("%~dp0"))?;
        let (rel, tail) = rest.split_once('"')?;
        if tail.trim() != "%*" || !rel.to_ascii_lowercase().ends_with(".exe") {
            return None;
        }
        let exe = rel
            .split('\\')
            .filter(|p| !p.is_empty())
            .fold(dir.to_path_buf(), |p, c| p.join(c));
        exe.is_file().then_some(exe)
    })
}

fn run_cli(bin: &Path, args: &[String]) -> Result<()> {
    let program = cmd_shim_target(bin).unwrap_or_else(|| bin.to_path_buf());
    let mut cmd = std::process::Command::new(&program);
    // The app runs this from a GUI process: no console window.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let o = cmd
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Error::io(bin, e))?;
    if o.status.success() {
        Ok(())
    } else {
        let err: String = String::from_utf8_lossy(&o.stderr)
            .trim()
            .chars()
            .take(300)
            .collect();
        Err(Error::Internal(format!(
            "`{} {}` failed: {err}",
            bin.display(),
            args.first().map(String::as_str).unwrap_or_default()
        )))
    }
}

#[cfg(test)]
mod cmd_shim_tests {
    use super::{HostEnv, cmd_shim_target};

    /// What `npm i -g @anthropic-ai/claude-code` writes (cmd-shim).
    const NATIVE_SHIM: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\"%dp0%\\node_modules\\@anthropic-ai\\claude-code\\bin\\claude.exe\"   %*\r\n";
    /// A shim that runs a script through node.
    const NODE_SHIM: &str = "@ECHO off\r\nSETLOCAL\r\nCALL :find_dp0\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n)\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\x\\cli.js\" %*\r\n";

    fn npm_dir(shim: &str, exe: bool) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("claude.cmd"), shim).unwrap();
        if exe {
            let bin = d.path().join("node_modules/@anthropic-ai/claude-code/bin");
            std::fs::create_dir_all(&bin).unwrap();
            std::fs::write(bin.join("claude.exe"), b"MZ").unwrap();
        }
        d
    }

    #[test]
    fn the_current_env_also_searches_the_usual_install_dirs() {
        let env = HostEnv::current();
        for dir in env.well_known_bin_dirs() {
            assert!(env.path.contains(&dir), "{}", dir.display());
        }
    }

    #[test]
    fn an_npm_shim_resolves_to_the_native_claude() {
        let d = npm_dir(NATIVE_SHIM, true);
        let exe = cmd_shim_target(&d.path().join("claude.cmd")).unwrap();
        assert!(exe.ends_with("node_modules/@anthropic-ai/claude-code/bin/claude.exe"));
        assert!(exe.is_file());
    }

    #[test]
    fn other_shims_and_programs_are_left_alone() {
        // The executable the shim names is missing.
        let d = npm_dir(NATIVE_SHIM, false);
        assert_eq!(cmd_shim_target(&d.path().join("claude.cmd")), None);
        // A node script, not an executable.
        let d = npm_dir(NODE_SHIM, false);
        assert_eq!(cmd_shim_target(&d.path().join("claude.cmd")), None);
        // Not a batch file.
        let d = npm_dir(NATIVE_SHIM, true);
        std::fs::write(d.path().join("claude.exe"), b"MZ").unwrap();
        assert_eq!(cmd_shim_target(&d.path().join("claude.exe")), None);
    }
}
