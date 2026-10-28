//! Whether each agent on this computer is really connected to Cua: the one
//! status the app (onboarding rows, the Agents page), the CLI (`cua connect
//! status`) and MCP (`connect_status`) show.
//!
//! Every field comes from a check made now, never from a flag saved when
//! Cua connected the agent:
//!
//! - **Installed**: a binary on PATH or an app bundle. A config folder left
//!   behind after an uninstall does not count.
//! - **Version**: `<binary> --version` (short timeout).
//! - **Signed in**: read-only checks of the agent's own sign-in files
//!   ([`crate::signin`]); `unknown` where Cua can't tell, never a guess.
//! - **MCP**: Cua's entry is in the agent's config, its command exists, and
//!   it answers an MCP `initialize` (cached per binary and mtime).
//! - **Sign-in copy**: the agent's sign-in is in Keyvault on This Mac
//!   ([`SignInVault`]) and not past its expiry.
//!
//! The answer is cached for a few seconds ([`CACHE_FOR`]); the app asks
//! again with `refresh` when its window gets focus.

use crate::{
    AgentSetup, HostEnv, McpServer, edit, is_cua_command, read_text, registry,
    signin::{self, SignedIn},
    state::State,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// The agents the status leads with, in the order every surface shows them.
pub const LEADING: [&str; 6] = [
    "hermes",
    "claude-code",
    "codex",
    "cursor",
    "openclaw",
    "t3code",
];

/// How long an answer is reused without `refresh`.
pub const CACHE_FOR: Duration = Duration::from_secs(5);

/// How long a working MCP command is trusted before it is asked again
/// (the binary's mtime changing asks at once).
const PROBE_FOR: Duration = Duration::from_secs(600);

/// `<bin> --version` and the MCP handshake give up after this.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether the agent's own sign-in is on this computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignedInState {
    Yes,
    No,
    Unknown,
    Expired,
}

impl From<SignedIn> for SignedInState {
    fn from(s: SignedIn) -> Self {
        match s {
            SignedIn::Yes => SignedInState::Yes,
            SignedIn::No => SignedInState::No,
            SignedIn::Unknown => SignedInState::Unknown,
            SignedIn::Expired => SignedInState::Expired,
        }
    }
}

/// Cua's MCP entry in the agent's config.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpState {
    /// Present, its command exists and answers.
    Ok,
    /// Not in the config.
    Missing,
    /// Present but its command is gone, the config can't be read, or the
    /// server doesn't answer.
    Broken,
    /// Present and working, but it launches another cua than this one.
    Stale,
    /// The agent has no MCP config Cua can write.
    Unsupported,
}

/// The copy of the agent's sign-in in Keyvault on This Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportState {
    Ok,
    Expired,
    Missing,
    /// Cua doesn't copy this agent's sign-in.
    Unsupported,
}

/// The one word a row leads with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    /// Everything Cua needs works.
    Connected,
    /// Something Cua can fix or the person must do: see `problem`.
    Attention,
    /// Installed, Cua was never added (or was removed by Disconnect).
    NotConnected,
    /// Not on this computer.
    NotInstalled,
}

/// What the row's one button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProblemAction {
    /// Write Cua's entry again (from the backup-aware editor) and copy the
    /// sign-in again.
    Reconnect,
    /// Start the agent's own sign-in.
    SignIn,
    /// Install the agent.
    Install,
    /// Take Cua's entry out of a config the agent no longer reads.
    Disconnect,
    /// Nothing to press (the line says what to do).
    None,
}

/// What is wrong, in one line, and the fix.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Problem {
    pub line: String,
    pub action: ProblemAction,
}

/// One agent's connection (`agents.status`' row; CONTRACT §4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionRow {
    pub id: String,
    pub name: String,
    pub installed: bool,
    pub version: Option<String>,
    /// The version Cua last connected (differs from `version` after an
    /// upgrade).
    pub previous_version: Option<String>,
    pub signed_in: SignedInState,
    /// What Connect copies ("ChatGPT sign-in"), when the agent has one.
    pub sign_in: Option<String>,
    pub mcp: McpState,
    /// The config file Cua's entry lives in (`~/.codex/config.toml`).
    pub mcp_config: Option<String>,
    pub signin_imported: bool,
    pub signin_import: ImportState,
    pub state: ConnectionState,
    pub problem: Option<Problem>,
    pub checked_at_ms: u64,
}

/// Every row, and when they were checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionReport {
    pub agents: Vec<ConnectionRow>,
    pub checked_at_ms: u64,
}

impl ConnectionReport {
    /// The row of `id`.
    pub fn row(&self, id: &str) -> Option<&ConnectionRow> {
        self.agents.iter().find(|r| r.id == id)
    }
}

/// A sign-in copied into Keyvault (what the store answers; never its value).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedSignIn {
    /// The agent it was copied from (setup id).
    pub source: String,
    /// The variable a run in a Space gets it as.
    pub env: String,
    /// "Codex · ChatGPT sign-in".
    pub label: String,
    /// When it stops working (Unix ms), when known.
    pub expires_ms: Option<u64>,
    pub added_ms: u64,
}

/// A sign-in to copy into Keyvault. `Debug` never prints the value.
#[derive(Clone, PartialEq, Eq)]
pub struct SignInCopy {
    pub source: String,
    pub env: String,
    pub label: String,
    pub value: String,
    pub expires_ms: Option<u64>,
}

impl std::fmt::Debug for SignInCopy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignInCopy")
            .field("source", &self.source)
            .field("env", &self.env)
            .field("label", &self.label)
            .field("value", &"<redacted>")
            .field("expires_ms", &self.expires_ms)
            .finish()
    }
}

/// Keyvault on This Mac, as the connection status and the sign-in import
/// see it. The app, the CLI and MCP pass the real one (the Keychain-backed
/// agent keys); tests pass a fake.
pub trait SignInVault: Send + Sync {
    /// The imported sign-ins.
    fn imported(&self) -> Result<Vec<ImportedSignIn>, String>;
    /// Saves a copy (replacing an earlier copy from the same agent).
    fn save(&self, copy: &SignInCopy) -> Result<ImportedSignIn, String>;
    /// Deletes every copy from `source`; the variables removed.
    fn remove(&self, source: &str) -> Result<Vec<String>, String>;
}

/// How to check.
#[derive(Clone, Debug)]
pub struct StatusOptions {
    /// The MCP server this app or CLI would write (its own cua). An entry
    /// launching another cua is `stale`. None: any cua binary is fine.
    pub expected: Option<McpServer>,
    /// Ignore the cached answer (window focus, after Connect).
    pub refresh: bool,
    /// `--version` and the handshake give up after this.
    pub timeout: Duration,
}

impl Default for StatusOptions {
    fn default() -> Self {
        StatusOptions {
            expected: None,
            refresh: false,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

type CacheKey = (PathBuf, PathBuf, Option<String>);

fn report_cache() -> &'static Mutex<HashMap<CacheKey, (Instant, ConnectionReport)>> {
    static C: OnceLock<Mutex<HashMap<CacheKey, (Instant, ConnectionReport)>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Forgets every cached answer (after Connect, Disconnect or an import).
pub fn invalidate() {
    if let Ok(mut c) = report_cache().lock() {
        c.clear();
    }
}

/// `<bin> --version`'s first version-looking word.
pub fn parse_version(out: &str) -> Option<String> {
    let line = out.lines().map(str::trim).find(|l| !l.is_empty())?;
    line.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',')
        .map(|w| w.trim_start_matches('v'))
        .find(|w| {
            let mut parts = w.split('.');
            let first = parts.next().unwrap_or_default();
            !first.is_empty()
                && first.chars().all(|c| c.is_ascii_digit())
                && parts
                    .next()
                    .is_some_and(|p| p.starts_with(|c: char| c.is_ascii_digit()))
        })
        .map(str::to_string)
}

pub(crate) fn base_command(env: &HostEnv, program: &Path) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // The agent and cua read their homes from HOME; a GUI app has no shell
    // PATH, so the dirs the agent was found in come along.
    cmd.env("HOME", &env.home);
    if let Ok(path) = std::env::join_paths(&env.path) {
        cmd.env("PATH", path);
    }
    for (k, v) in &env.vars {
        cmd.env(k, v);
    }
    cmd
}

/// Runs `program args` and returns stdout if it exits 0 within `timeout`.
/// The child is killed and reaped on timeout.
pub(crate) fn run_with_timeout(
    env: &HostEnv,
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Option<String> {
    match run_capture(env, program, args, timeout)? {
        (Some(0), out) => Some(out),
        _ => None,
    }
}

/// Runs `program args` with stdin closed; its exit code and stdout, or
/// None when it can't start or outlives `timeout` (then it is killed and
/// reaped).
pub(crate) fn run_capture(
    env: &HostEnv,
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> Option<(Option<i32>, String)> {
    let mut child = base_command(env, program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout, &mut s);
        String::from_utf8_lossy(&s).into_owned()
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let out = reader.join().unwrap_or_default();
                return Some((status.code(), out));
            }
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// A probed command and its arguments.
type ProbeKey = (PathBuf, Vec<String>);
/// When it was probed, the binary's mtime then, and whether it answered.
type ProbeAnswer = (Instant, Option<SystemTime>, bool);

fn probe_cache() -> &'static Mutex<HashMap<ProbeKey, ProbeAnswer>> {
    static C: OnceLock<Mutex<HashMap<ProbeKey, ProbeAnswer>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Whether `command args` answers an MCP `initialize` with a result within
/// `timeout`. Only `initialize` is sent: no tool runs, nothing is logged as
/// an agent using Cua. A working answer is reused for [`PROBE_FOR`] while
/// the binary's mtime stays the same.
pub fn mcp_answers(env: &HostEnv, command: &Path, args: &[String], timeout: Duration) -> bool {
    let key = (command.to_path_buf(), args.to_vec());
    let mtime = std::fs::metadata(command).and_then(|m| m.modified()).ok();
    if let Ok(c) = probe_cache().lock()
        && let Some((at, m, ok)) = c.get(&key)
        && *ok
        && *m == mtime
        && at.elapsed() < PROBE_FOR
    {
        return true;
    }
    let ok = handshake(env, command, args, timeout);
    if let Ok(mut c) = probe_cache().lock() {
        c.insert(key, (Instant::now(), mtime, ok));
    }
    ok
}

fn handshake(env: &HostEnv, command: &Path, args: &[String], timeout: Duration) -> bool {
    let Ok(mut child) = base_command(env, command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let init = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            // Named so the server never takes it for an agent.
            "clientInfo": {"name": "cua-connect-status", "version": env!("CARGO_PKG_VERSION")}
        }
    });
    let mut stdin = child.stdin.take();
    if let Some(s) = stdin.as_mut() {
        let _ = writeln!(s, "{init}");
        let _ = s.flush();
    }
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(out) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
                    continue;
                };
                if v.get("id") == Some(&serde_json::json!(1)) {
                    let _ = tx.send(v.get("result").is_some());
                    return;
                }
            }
            let _ = tx.send(false);
        });
    }
    let ok = rx.recv_timeout(timeout).unwrap_or(false);
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    ok
}

/// A path as the person reads it: `~/x` under the home.
pub(crate) fn tilde(env: &HostEnv, p: &Path) -> String {
    match p.strip_prefix(&env.home) {
        Ok(rel) => {
            let parts: Vec<_> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            format!("~/{}", parts.join("/"))
        }
        Err(_) => p.display().to_string(),
    }
}

/// Resolves a launch command the way the agent will: an absolute path as
/// is, a bare name on the agent's PATH.
fn resolve(env: &HostEnv, cmd: &str) -> Option<PathBuf> {
    let p = Path::new(cmd);
    if p.is_absolute() || cmd.contains('/') {
        let p = if let Some(rest) = cmd.strip_prefix("~/") {
            env.home.join(rest)
        } else {
            p.to_path_buf()
        };
        return crate::is_executable(&p).then_some(p);
    }
    env.which(cmd)
}

/// The installed binary of `a`, first on PATH.
pub(crate) fn binary(env: &HostEnv, a: &registry::AgentSpec) -> Option<PathBuf> {
    a.bins.iter().find_map(|b| env.which(b))
}

fn app_bundle(env: &HostEnv, a: &registry::AgentSpec) -> Option<PathBuf> {
    if env.os != crate::Os::MacOs {
        return None;
    }
    a.apps
        .iter()
        .flat_map(|app| env.app_dirs.iter().map(move |d| d.join(app)))
        .find(|p| p.is_dir())
}

impl AgentSetup {
    /// Every leading agent's connection, then any other agent that is
    /// installed or still has Cua's entry. Cached for [`CACHE_FOR`] unless
    /// `opts.refresh`.
    pub fn connection_status(
        &self,
        opts: &StatusOptions,
        vault: Option<&dyn SignInVault>,
    ) -> ConnectionReport {
        let key: CacheKey = (
            self.env.home.clone(),
            self.state_path.clone(),
            opts.expected.as_ref().map(|s| s.command.clone()),
        );
        if !opts.refresh
            && let Ok(c) = report_cache().lock()
            && let Some((at, r)) = c.get(&key)
            && at.elapsed() < CACHE_FOR
        {
            return r.clone();
        }
        let report = self.check_connections(opts, vault);
        if let Ok(mut c) = report_cache().lock() {
            c.insert(key, (Instant::now(), report.clone()));
        }
        report
    }

    fn check_connections(
        &self,
        opts: &StatusOptions,
        vault: Option<&dyn SignInVault>,
    ) -> ConnectionReport {
        let state = State::load(&self.state_path).unwrap_or_default();
        let imported = vault.map(|v| v.imported());
        let now = now_ms();
        let mut specs: Vec<&registry::AgentSpec> =
            LEADING.iter().filter_map(|id| registry::find(id)).collect();
        // Checked in parallel: each may wait on a `--version` or handshake.
        let rows: Vec<ConnectionRow> = std::thread::scope(|s| {
            let mut handles = Vec::new();
            for a in registry::AGENTS {
                if !specs.iter().any(|x| x.id == a.id) {
                    specs.push(a);
                }
            }
            for a in &specs {
                let state = &state;
                let imported = imported.as_ref();
                handles.push(s.spawn(move || self.check_one(a, state, imported, opts, now)));
            }
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        let agents = rows
            .into_iter()
            .filter(|r| {
                LEADING.contains(&r.id.as_str())
                    || r.installed
                    || matches!(r.mcp, McpState::Ok | McpState::Broken | McpState::Stale)
            })
            .collect();
        ConnectionReport {
            agents,
            checked_at_ms: now,
        }
    }

    fn check_one(
        &self,
        a: &registry::AgentSpec,
        state: &State,
        imported: Option<&Result<Vec<ImportedSignIn>, String>>,
        opts: &StatusOptions,
        now: u64,
    ) -> ConnectionRow {
        let env = &self.env;
        let bin = binary(env, a);
        let installed = bin.is_some() || app_bundle(env, a).is_some();
        let version = bin
            .as_ref()
            .and_then(|b| run_with_timeout(env, b, &["--version"], opts.timeout))
            .and_then(|o| parse_version(&o));
        let record = state.connections.get(a.id);
        let previous_version = record.and_then(|r| r.version.clone());

        // MCP.
        let own = env.channel.mcp_server_name();
        let (mut mcp, mut mcp_config, mut mcp_problem) = (McpState::Unsupported, None, None);
        let mut cua_was_here = false;
        if let Some(m) = &a.mcp {
            let file = self.mcp_file(m);
            let shown = tilde(env, &file);
            cua_was_here = state
                .record(own, &file)
                .is_some_and(|r| r.agents.contains(a.id))
                || record.is_some();
            mcp_config = Some(shown.clone());
            (mcp, mcp_problem) = match read_text(&file)
                .and_then(|t| edit::get(m.format, &file, &t, m.key_path, own))
            {
                Err(e) => (
                    McpState::Broken,
                    Some(format!(
                        "{shown} can't be read ({}). Fix the file, then Reconnect.",
                        short(&e.to_string())
                    )),
                ),
                Ok(None) => (McpState::Missing, None),
                // An entry of that name launching something that isn't
                // a cua (and that Cua didn't write) is someone else's.
                Ok(Some(v))
                    if !m.shape.command_of(&v).is_some_and(|(c, _)| {
                        is_cua_command(&c, env.channel)
                            || opts.expected.as_ref().is_some_and(|e| e.command == c)
                    }) && !state.record(own, &file).is_some_and(|r| r.value == v) =>
                {
                    (McpState::Missing, None)
                }
                Ok(Some(v)) => match m.shape.command_of(&v) {
                    None => (
                        McpState::Broken,
                        Some(format!(
                            "Cua's entry in {shown} has no command. Reconnect fixes it."
                        )),
                    ),
                    Some((cmd, args)) => match resolve(env, &cmd) {
                        None => (
                            McpState::Broken,
                            Some(format!(
                                "Cua's entry in {shown} points at a cua that isn't there. Reconnect fixes it."
                            )),
                        ),
                        Some(path) if !mcp_answers(env, &path, &args, opts.timeout) => (
                            McpState::Broken,
                            Some(format!(
                                "Cua's tools in {} don't answer. Reconnect fixes it.",
                                display_name(a)
                            )),
                        ),
                        Some(path) => {
                            let ours = match &opts.expected {
                                None => is_cua_command(&cmd, env.channel),
                                Some(want) => {
                                    resolve(env, &want.command)
                                        .is_some_and(|w| same_file(&w, &path))
                                        && args == want.args
                                }
                            };
                            if ours {
                                (McpState::Ok, None)
                            } else {
                                (
                                    McpState::Stale,
                                    Some(format!(
                                        "{} uses another copy of cua. Reconnect to use this one.",
                                        display_name(a)
                                    )),
                                )
                            }
                        }
                    },
                },
            };
        }

        // The agent's own sign-in, and the copy in Keyvault.
        let own_sign_in = signin::check(env, a.id);
        let signed_in: SignedInState = own_sign_in.state.into();
        let supported = signin::importable(a.id);
        let copy = imported
            .and_then(|r| r.as_ref().ok())
            .and_then(|list| list.iter().find(|i| i.source == a.id));
        let signin_import = match (supported, copy) {
            (false, None) => ImportState::Unsupported,
            (_, None) => ImportState::Missing,
            (_, Some(c)) if c.expires_ms.is_some_and(|e| e <= now) => ImportState::Expired,
            (_, Some(_)) => ImportState::Ok,
        };

        // The one word, and the one line.
        let name = display_name(a);
        let (state_word, problem) = if !installed {
            if matches!(mcp, McpState::Ok | McpState::Broken | McpState::Stale) {
                (
                    ConnectionState::Attention,
                    Some(Problem {
                        line: format!(
                            "{name} isn't on This Mac anymore. Disconnect takes Cua out of its settings."
                        ),
                        action: ProblemAction::Disconnect,
                    }),
                )
            } else {
                (ConnectionState::NotInstalled, None)
            }
        } else {
            match mcp {
                McpState::Missing if cua_was_here => (
                    ConnectionState::Attention,
                    Some(Problem {
                        line: format!(
                            "Cua was taken out of {}. Reconnect adds it back.",
                            mcp_config
                                .clone()
                                .unwrap_or_else(|| format!("{name}'s settings"))
                        ),
                        action: ProblemAction::Reconnect,
                    }),
                ),
                McpState::Missing => (ConnectionState::NotConnected, None),
                McpState::Broken => (
                    ConnectionState::Attention,
                    Some(Problem {
                        line: mcp_problem.clone().unwrap_or_default(),
                        action: ProblemAction::Reconnect,
                    }),
                ),
                McpState::Unsupported if record.is_none() => (ConnectionState::NotConnected, None),
                McpState::Ok | McpState::Stale | McpState::Unsupported => {
                    match (signed_in, own_sign_in.label) {
                        (SignedInState::Expired, label) => (
                            ConnectionState::Attention,
                            Some(Problem {
                                line: format!(
                                    "{name}'s {} expired. Sign in again.",
                                    label.unwrap_or("sign-in")
                                ),
                                action: if signin::can_start_sign_in(a.id) {
                                    ProblemAction::SignIn
                                } else {
                                    ProblemAction::None
                                },
                            }),
                        ),
                        (SignedInState::No, label) => (
                            ConnectionState::Attention,
                            Some(Problem {
                                line: match label {
                                    Some(l) => format!("Connected. No {l} on This Mac yet."),
                                    None => format!(
                                        "Connected. {name} isn't signed in on This Mac yet."
                                    ),
                                },
                                action: if signin::can_start_sign_in(a.id) {
                                    ProblemAction::SignIn
                                } else {
                                    ProblemAction::None
                                },
                            }),
                        ),
                        _ if signin_import == ImportState::Expired => (
                            ConnectionState::Attention,
                            Some(Problem {
                                line: format!(
                                    "The copy of {name}'s sign-in in Keyvault expired. Reconnect copies it again."
                                ),
                                action: ProblemAction::Reconnect,
                            }),
                        ),
                        _ if mcp == McpState::Stale => (
                            ConnectionState::Connected,
                            Some(Problem {
                                line: mcp_problem.clone().unwrap_or_default(),
                                action: ProblemAction::Reconnect,
                            }),
                        ),
                        _ => (ConnectionState::Connected, None),
                    }
                }
            }
        };

        ConnectionRow {
            id: a.id.into(),
            name: display_name(a).into(),
            installed,
            version,
            previous_version,
            signed_in,
            sign_in: signin::copy_label(env, a.id),
            mcp,
            mcp_config,
            signin_imported: copy.is_some(),
            signin_import,
            state: state_word,
            problem,
            checked_at_ms: now,
        }
    }
}

/// What copying one agent's sign-in did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInResult {
    /// The copy now in Keyvault.
    pub imported: Option<ImportedSignIn>,
    /// Why there is none: `not_signed_in`, `cancelled`, `not_yet`, `failed`.
    pub refused: Option<String>,
    /// The row's one line when refused.
    pub line: Option<String>,
}

/// What Connect (or Reconnect) did for one agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConnectResult {
    pub agent: String,
    /// Every skill and MCP change.
    pub outcomes: Vec<crate::Outcome>,
    /// Cua's tools are in the agent's settings now.
    pub tools_ok: bool,
    /// The one line when they aren't ("Couldn't add Cua to ~/.claude.json").
    pub tools_line: Option<String>,
    /// The sign-in copy, when asked for.
    pub sign_in: Option<SignInResult>,
}

/// What Connect does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectParts {
    pub skills: bool,
    pub mcp: bool,
    /// Copy the agent's sign-in into Keyvault.
    pub sign_in: bool,
}

impl ConnectParts {
    /// Simple's Connect: tools and the sign-in.
    pub const ALL: ConnectParts = ConnectParts {
        skills: true,
        mcp: true,
        sign_in: true,
    };
}

impl AgentSetup {
    /// Notes that Cua connected `agent` now (its version, for "upgraded").
    pub(crate) fn record_connected(&self, state: &mut State, agent: &str) {
        let version = registry::find(agent)
            .and_then(|a| binary(&self.env, a))
            .and_then(|b| run_with_timeout(&self.env, &b, &["--version"], DEFAULT_TIMEOUT))
            .and_then(|o| parse_version(&o));
        let r = state.connections.entry(agent.to_string()).or_default();
        r.version = version;
        r.connected_at = crate::state::now();
    }

    /// Copies `agent`'s sign-in into `vault` (Claude Code waits up to
    /// `wait` for its one browser approval). The agent's own sign-in is
    /// never changed.
    pub fn import_sign_in(
        &self,
        agent: &str,
        vault: &dyn SignInVault,
        wait: Duration,
    ) -> std::result::Result<ImportedSignIn, signin::CopyRefusal> {
        let a = registry::find(agent)
            .ok_or_else(|| signin::CopyRefusal::Failed(signin::unknown(agent).to_string()))?;
        let copy = signin::read_copy(&self.env, a.id, wait)?;
        self.save_sign_in(a.id, &copy, vault)
    }

    /// Saves a sign-in made for `agent` into `vault` and notes it.
    fn save_sign_in(
        &self,
        agent: &str,
        copy: &SignInCopy,
        vault: &dyn SignInVault,
    ) -> std::result::Result<ImportedSignIn, signin::CopyRefusal> {
        let saved = vault
            .save(copy)
            .map_err(|e| signin::CopyRefusal::Failed(format!("Couldn't save the sign-in: {e}")))?;
        if let Ok(mut state) = State::load(&self.state_path) {
            let r = state.connections.entry(agent.to_string()).or_default();
            r.sign_in = Some(crate::state::SignInRecord {
                env: saved.env.clone(),
                label: saved.label.clone(),
                copied_at: crate::state::now(),
            });
            let _ = state.save(&self.state_path);
        }
        invalidate();
        Ok(saved)
    }

    /// Starts `agent`'s device-code sign-in (Codex: `codex login
    /// --device-auth`) in a folder Cua owns, next to its state; the link and
    /// code to show. Your own sign-in is never read or changed.
    pub fn start_device_sign_in(
        &self,
        agent: &str,
    ) -> std::result::Result<signin::DeviceSignIn, signin::CopyRefusal> {
        let dir = self
            .state_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("agent-sign-ins")
            .join(agent);
        signin::start_device_sign_in(&self.env, &dir, agent)
    }

    /// Waits for a device sign-in to be approved and saves it into `vault`.
    pub fn finish_device_sign_in(
        &self,
        login: signin::DeviceSignIn,
        vault: &dyn SignInVault,
        wait: Duration,
    ) -> std::result::Result<ImportedSignIn, signin::CopyRefusal> {
        let copy = login.wait(wait)?;
        let agent = copy.source.clone();
        self.save_sign_in(&agent, &copy, vault)
    }

    /// The whole device sign-in for `agent`: start it, `show` the link and
    /// code, wait (up to `wait`) for the approval, save it into `vault`.
    pub fn device_sign_in(
        &self,
        agent: &str,
        vault: &dyn SignInVault,
        wait: Duration,
        show: ShowDeviceSignIn<'_>,
    ) -> std::result::Result<ImportedSignIn, signin::CopyRefusal> {
        let login = self.start_device_sign_in(agent)?;
        show(agent, &login);
        self.finish_device_sign_in(login, vault, wait)
    }

    /// Deletes the copies of `agent`'s sign-in from `vault`; the variables
    /// removed.
    pub fn remove_sign_in(
        &self,
        agent: &str,
        vault: &dyn SignInVault,
    ) -> crate::Result<Vec<String>> {
        let a = registry::find(agent).ok_or_else(|| signin::unknown(agent))?;
        let removed = vault.remove(a.id).map_err(crate::Error::Internal)?;
        if let Ok(mut state) = State::load(&self.state_path)
            && let Some(r) = state.connections.get_mut(a.id)
        {
            r.sign_in = None;
            let _ = state.save(&self.state_path);
        }
        invalidate();
        Ok(removed)
    }

    /// Connect (and Reconnect): Cua's skills and MCP entry through the
    /// backup-aware editor, then the sign-in copy. A failed sign-in copy
    /// leaves the tools connected.
    pub fn connect(
        &self,
        agents: &[String],
        server: &McpServer,
        parts: ConnectParts,
        vault: Option<&dyn SignInVault>,
        wait: Duration,
    ) -> crate::Result<Vec<ConnectResult>> {
        self.connect_with(agents, server, parts, vault, wait, None)
    }

    /// [`Self::connect`], and an agent that signs in with a code (Codex)
    /// does so here: its device sign-in starts in a folder Cua owns,
    /// `show` gets the link and the code to enter, and the sign-in made for
    /// Cua is saved once it's approved. Without `show` such
    /// an agent's sign-in is left for the app's Connect, as before. The
    /// device sign-in waits up to [`signin::DEVICE_SIGN_IN_WAIT`]. The CLI's
    /// `connect setup --sign-in` and the app take this same path.
    pub fn connect_with(
        &self,
        agents: &[String],
        server: &McpServer,
        parts: ConnectParts,
        vault: Option<&dyn SignInVault>,
        wait: Duration,
        mut show: Option<ShowDeviceSignIn<'_>>,
    ) -> crate::Result<Vec<ConnectResult>> {
        let mut out = Vec::new();
        for id in agents {
            let a = registry::find(id).ok_or_else(|| signin::unknown(id))?;
            let ids = vec![a.id.to_string()];
            let mut outcomes = Vec::new();
            if parts.skills && a.skills.is_some() {
                outcomes.extend(self.install_skills(&ids, &[], false)?);
            }
            if parts.mcp {
                outcomes.extend(self.configure_mcp(&ids, server)?);
            }
            let failed = outcomes
                .iter()
                .find(|o| o.target == crate::Target::Mcp && o.failed());
            let tools_ok = failed.is_none();
            let tools_line =
                failed.map(|o| format!("Couldn't add Cua to {}", tilde(&self.env, &o.path)));
            let sign_in = (parts.sign_in && signin::importable(a.id)).then(|| match vault {
                None => SignInResult {
                    imported: None,
                    refused: Some("failed".into()),
                    line: Some("Couldn't save the sign-in: Keyvault isn't available here.".into()),
                },
                Some(v) => match match show.as_deref_mut() {
                    Some(show) if signin::signs_in_with_a_code(a.id) => {
                        self.device_sign_in(a.id, v, signin::DEVICE_SIGN_IN_WAIT, show)
                    }
                    _ => self.import_sign_in(a.id, v, wait),
                } {
                    Ok(i) => SignInResult {
                        imported: Some(i),
                        refused: None,
                        line: None,
                    },
                    Err(r) => SignInResult {
                        imported: None,
                        refused: Some(r.kind().into()),
                        line: Some(r.line().into()),
                    },
                },
            });
            out.push(ConnectResult {
                agent: a.id.into(),
                outcomes,
                tools_ok,
                tools_line,
                sign_in,
            });
        }
        invalidate();
        Ok(out)
    }

    /// Disconnect: Cua's entry out of the agent's config (the entry that
    /// was there before comes back), its skills, and with `sign_in` the
    /// copy in Keyvault.
    pub fn disconnect(
        &self,
        agents: &[String],
        parts: crate::Parts,
        opts: crate::RemoveOptions,
        sign_in: bool,
        vault: Option<&dyn SignInVault>,
    ) -> crate::Result<(Vec<crate::Outcome>, Vec<String>)> {
        let outcomes = self.remove_with(agents, parts, opts)?;
        let mut removed = Vec::new();
        if opts.dry_run {
            return Ok((outcomes, removed));
        }
        if sign_in && let Some(v) = vault {
            for a in agents {
                removed.extend(self.remove_sign_in(a, v)?);
            }
        }
        invalidate();
        Ok((outcomes, removed))
    }
}

/// Shows a device sign-in's link and code (`agent`, the sign-in) while it
/// waits for the approval.
pub type ShowDeviceSignIn<'a> = &'a mut dyn FnMut(&str, &signin::DeviceSignIn);

/// The name rows show ("Codex", not the registry's "OpenAI Codex").
pub fn display_name(a: &registry::AgentSpec) -> &'static str {
    match a.id {
        "codex" => "Codex",
        _ => a.name,
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

fn short(s: &str) -> String {
    let s = s.lines().next().unwrap_or_default();
    if s.chars().count() > 120 {
        format!("{}…", s.chars().take(119).collect::<String>())
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn versions_are_read_from_each_agents_own_words() {
        assert_eq!(
            parse_version("codex-cli 0.61.0\n").as_deref(),
            Some("0.61.0")
        );
        assert_eq!(
            parse_version("2.4.1 (Claude Code)").as_deref(),
            Some("2.4.1")
        );
        assert_eq!(
            parse_version("Hermes Agent v0.9.2 (2026.10.1)").as_deref(),
            Some("0.9.2")
        );
        assert_eq!(
            parse_version("2026.10.21-1a2b3c").as_deref(),
            Some("2026.10.21-1a2b3c")
        );
        assert_eq!(
            parse_version("\n  openclaw 2026.10.3\n").as_deref(),
            Some("2026.10.3")
        );
        assert_eq!(parse_version("no version here"), None);
        assert_eq!(parse_version(""), None);
    }
}
