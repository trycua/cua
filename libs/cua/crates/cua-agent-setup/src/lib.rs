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
        let path = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let os = Os::current();
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
        HostEnv {
            home,
            vars,
            path,
            app_dirs,
            os,
            run_agent_clis: true,
        }
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
        }
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

    /// `~/.cua` (or `$CUA_HOME`).
    pub fn cua_home(&self) -> PathBuf {
        self.var("CUA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.home.join(".cua"))
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

    /// `cua mcp`, with `cua` resolved to an absolute path on `env`'s PATH
    /// (GUI agents often run without the user's shell PATH), else `cua`.
    pub fn default_for(env: &HostEnv) -> Self {
        let cmd = env
            .which("cua")
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "cua".into());
        McpServer::new(cmd)
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

/// Whether a launch command is a cua binary (`cua`, `/x/bin/cua`,
/// `cua.exe`).
fn is_cua_command(cmd: &str) -> bool {
    let base = cmd.rsplit(['/', '\\']).next().unwrap_or(cmd);
    base == "cua" || base == "cua.exe"
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

    /// The bundled default skills.
    pub fn bundled_skills(&self) -> Vec<Skill> {
        skills::bundled()
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
        let bundled = skills::bundled();
        AGENTS
            .iter()
            .map(|a| self.detect_one(a, &state, &bundled))
            .collect()
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
        };
        if let Some(m) = &a.mcp {
            let file = self.mcp_file(m);
            status.mcp_format = Some(m.format);
            match read_text(&file)
                .and_then(|t| edit::get(m.format, &file, &t, m.key_path, MCP_SERVER_NAME))
            {
                Ok(Some(v)) => {
                    status.cua_configured = m
                        .shape
                        .command_of(&v)
                        .is_some_and(|(c, _)| is_cua_command(&c));
                    status.cua_managed = state.mcp.get(&file).is_some_and(|r| r.value == v);
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
        let bundled = skills::bundled();
        let wanted: Vec<&Skill> = if skill_names.is_empty() {
            bundled.iter().collect()
        } else {
            skill_names
                .iter()
                .map(|n| {
                    bundled
                        .iter()
                        .find(|s| &s.name == n)
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
            if let Err(e) = skills::write_to(&s.name, dest) {
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
                .and_then(|c| self.env.which(c.bin()).map(|bin| (c, bin)));
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
                    .is_some_and(|(c, _)| is_cua_command(&c))
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
                // `cua` always gets a row; other servers (`cua-driver`) only
                // when cua registered them there.
                let mut names = state.names_in(&file);
                if !state.mcp.contains_key(&file) {
                    names.insert(0, MCP_SERVER_NAME.to_string());
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
            let names: Vec<String> = names
                .into_iter()
                .filter(|n| skills::get(n).is_some())
                .collect();
            if known.is_empty() || names.is_empty() {
                continue;
            }
            out.extend(self.install_skills(&known, &names, false)?);
        }
        if let Some(server) = server {
            let agents: Vec<String> = state
                .mcp
                .values()
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
                let recorded = state.mcp.get(&file).map(|r| r.value.clone());
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

fn run_cli(bin: &Path, args: &[String]) -> Result<()> {
    let o = std::process::Command::new(bin)
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
