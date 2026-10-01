//! Agent runs inside one sandbox.
//!
//! A run is one detached cua-spacesd process (tag `cua-agent/<run>`) that
//! installs what the harness needs, then runs the ACP runner
//! (`runner/acp-runner.mjs`), which drives the harness's ACP agent. Every
//! fact about a run is a file under `$HOME/.cua/agents/<run>/` in the guest,
//! so a run outlives the client that started it, and any client can list,
//! read, continue or stop it later:
//!
//! | file            | written by | what                                         |
//! |-----------------|------------|----------------------------------------------|
//! | `meta.json`     | SDK        | harness, prompt, cwd, created_at, label      |
//! | `run.json`      | SDK        | what the runner runs (no secrets)            |
//! | `secrets.json`  | SDK        | env for the agent only, mode 0600            |
//! | `launch.sh`     | SDK        | install, clone, then `exec` the runner       |
//! | `events.jsonl`  | runner     | the normalized event log ([`crate::events`]) |
//! | `state.json`    | runner     | status, turn, ACP session id                 |
//! | `inbox/*.json`  | SDK        | follow-ups, interrupts, stop                 |
//!
//! The run directory is created with umask 077. Secrets never reach
//! `run.json`, `meta.json`, argv, or the event log (the runner redacts them).

use crate::events::{self, AgentEvent, EventPage};
use crate::harness::{self, Endpoint, Harness};
use crate::{Error, Result, installables, quote};
use cua_spacesd_client::{Command, DownloadOptions, SpacesdClient, UploadOptions, pb};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Run records directory under the guest user's home.
pub const RUNS_DIR: &str = ".cua/agents";
/// Tag prefix of run processes.
pub const TAG_PREFIX: &str = "cua-agent/";
/// The runner, uploaded into the guest.
pub const RUNNER: &str = include_str!("../runner/acp-runner.mjs");
/// The host bridge pipe (stdio MCP server of persistent agents), uploaded
/// into the guest.
pub const BRIDGE: &str = include_str!("../runner/bridge.mjs");
/// Persistent agent homes, under the guest user's home (`<home>/<name>`).
pub const HOMES_DIR: &str = "cua-volume/agents";
/// The bridge directory inside a run directory.
pub const BRIDGE_DIR: &str = "bridge";
/// The MCP server name of the host bridge.
pub const BRIDGE_SERVER: &str = "cua";
/// Largest events page read at once.
pub const PAGE_BYTES: u64 = 512 * 1024;
const RS: &str = "\x1e--cua--\x1e";

/// An MCP server the agent gets for the run (ACP `session/new`
/// `mcpServers`): remote over HTTP, or a stdio command in the sandbox.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServer {
    pub name: String,
    /// `http(s)://` URL (streamable HTTP).
    #[serde(default)]
    pub url: Option<String>,
    /// Header values are secrets: they travel in `secrets.json`.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// A command in the sandbox (stdio), instead of `url`.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// A file attached to the first prompt, uploaded into the run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attachment {
    /// File name inside the run's `attachments/` directory.
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Options for [`Agents::start`]. Everything is optional.
#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Working directory (default: the run's own `work/`).
    pub cwd: Option<String>,
    /// Clone this repository into the working directory first (in the
    /// sandbox, so it survives a disconnect).
    pub repo: Option<String>,
    /// Branch or tag for `repo`.
    pub branch: Option<String>,
    /// Files for the first prompt (ACP `resource_link`s).
    pub files: Vec<Attachment>,
    /// Env for the agent only: API keys and tokens (`ANTHROPIC_API_KEY`, ...).
    pub env: BTreeMap<String, String>,
    /// Extra MCP servers.
    pub mcp_servers: Vec<McpServer>,
    /// Give the agent the sandbox's own MCP (cua-driver through cua-spacesd).
    /// Default true when the guest serves one.
    pub sandbox_mcp: Option<bool>,
    /// Copy the cua skills into the harness's skills directory. Default true.
    pub skills: Option<bool>,
    /// A custom model endpoint.
    pub endpoint: Option<Endpoint>,
    /// Model id (also applied without an endpoint where the harness allows).
    pub model: Option<String>,
    /// Install what the harness needs. Default true.
    pub install: Option<bool>,
    /// Exit (resumably) once the queue is empty: fire-and-forget batch runs.
    pub exit_when_idle: bool,
    /// Exit after this long without a prompt. Default 1800.
    pub idle_exit_secs: Option<u32>,
    /// A label for listings.
    pub label: Option<String>,
    /// Run as the persistent agent with this name: harness memory lives in
    /// its home (`<guest home>/cua-volume/agents/<name>`, see
    /// [`crate::harness::memory_dir`]) and the working directory defaults to
    /// `<agent home>/work`. The caller moves the home in and out.
    pub home: Option<String>,
    /// The home's guest directory when it is not the default: the agent's
    /// folder on the Space's mounted Cua Volume, which needs no moving.
    /// Used with `home`.
    pub home_dir: Option<String>,
    /// Give the agent the host bridge (MCP server `cua`: a pipe the host
    /// answers as the persistent agent, see `runner/bridge.mjs`).
    pub bridge: bool,
}

/// `meta.json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunMeta {
    pub run_id: String,
    pub harness: String,
    pub prompt: String,
    pub cwd: String,
    /// Unix seconds.
    pub created_at: f64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    /// The persistent agent this run belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
}

/// One status vocabulary for every harness. `idle` means "no turn is
/// running and a follow-up is accepted" (a runner that exited is restarted
/// and resumes its ACP session).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    /// Installing, starting, or a turn is running.
    Running,
    /// Waiting for a follow-up.
    Idle,
    /// The run failed (install, auth, or the agent could not start).
    Failed,
    /// The process is gone without recording why.
    Crashed,
    /// The sandbox could not be read. Never a guess.
    Unknown,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            RunStatus::Running => "running",
            RunStatus::Idle => "idle",
            RunStatus::Failed => "failed",
            RunStatus::Crashed => "crashed",
            RunStatus::Unknown => "unknown",
        }
    }
}

/// Whether a follow-up sent now starts the next turn: the run waits for
/// one (`idle`, including a runner that exited cleanly), or its process is
/// gone and [`Agents::send`] restarts it to resume the session (`crashed`,
/// `failed`). Not while a turn is running: `agent_message` would queue it
/// behind that turn (or, with `force`, cancel the turn), and a client that
/// opens a new turn on delivery would mix the two. Not when the run could
/// not be read. This is the one rule; SDKs and apps read the published
/// `accepts_message` instead of deriving it from `status`.
pub fn accepts_message(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Idle | RunStatus::Crashed | RunStatus::Failed
    )
}

/// A run, as read from the sandbox.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunInfo {
    pub run_id: String,
    pub harness: Option<String>,
    pub status: RunStatus,
    /// Finer grain: `installing`, `starting`, `working`, `waiting`,
    /// `exited`, `failed`, `crashed`, `unknown`.
    pub phase: String,
    pub reason: String,
    pub turn: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Is the run's process alive? `None`: could not tell.
    pub alive: Option<bool>,
    pub accepts_message: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<RunMeta>,
}

/// What a finished (or current) turn produced.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunResult {
    pub run_id: String,
    pub status: RunStatus,
    pub turn: u32,
    /// The agent's messages in the last turn, joined.
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Tool calls in the last turn.
    pub tool_calls: u32,
}

/// A file the run changed or created.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Artifact {
    pub path: String,
    pub size: u64,
    /// Unix milliseconds.
    pub modified_ms: u64,
}

/// What [`Agents::start`] returns.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Started {
    pub run_id: String,
    pub harness: String,
    pub process_tag: String,
    pub cwd: String,
    pub run_dir: String,
    /// Every preparation step that could not be completed, named.
    pub notes: Vec<String>,
}

/// The guest's own MCP endpoint (cua-spacesd `/mcp`, cua-driver tools).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpacesdMcp {
    pub url: String,
    pub token: Option<String>,
}

/// Header naming the agent run on the guest MCP: cua-spacesd keys the
/// cua-driver session (and so the agent cursor) by it, so every MCP session
/// the harness opens during the run is one cursor.
pub const AGENT_SESSION_HEADER: &str = "X-Cua-Agent-Session";

impl SpacesdMcp {
    /// The `cua-driver` MCP server of run `run_id`.
    pub fn server_for(&self, run_id: &str) -> McpServer {
        let mut headers = BTreeMap::new();
        if let Some(t) = &self.token {
            headers.insert("Authorization".into(), format!("Bearer {t}"));
        }
        headers.insert(AGENT_SESSION_HEADER.into(), run_id.to_owned());
        McpServer {
            name: "cua-driver".into(),
            url: Some(self.url.clone()),
            headers,
            ..Default::default()
        }
    }
}

/// Agent runs in one sandbox.
#[derive(Clone)]
pub struct Agents {
    guest: SpacesdClient,
    home: String,
    spacesd_mcp: Option<SpacesdMcp>,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn valid_run_id(run_id: &str) -> Result<()> {
    if run_id.is_empty()
        || run_id.len() > 64
        || !run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(Error::Invalid(format!("not a run id: {run_id:?}")));
    }
    Ok(())
}

/// A persistent agent name (the Cua Volume rule): 1 to 63 of `[a-z0-9._-]`,
/// starting with a letter or digit.
pub fn valid_home_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
}

fn valid_env_name(k: &str) -> Result<()> {
    if k.is_empty()
        || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || k.as_bytes()[0].is_ascii_digit()
    {
        return Err(Error::Invalid(format!("not an env var name: {k:?}")));
    }
    Ok(())
}

/// The run's status from its state file and process liveness.
pub fn classify(state: &Value, alive: Option<bool>) -> (RunStatus, String, String) {
    let st = state.get("status").and_then(Value::as_str).unwrap_or("");
    let err = state.get("error").and_then(Value::as_str);
    match (st, alive) {
        (_, None) => (
            RunStatus::Unknown,
            "unknown".into(),
            "could not tell whether the run's process is alive".into(),
        ),
        ("failed", _) => (
            RunStatus::Failed,
            "failed".into(),
            format!("the run failed: {}", err.unwrap_or("see the event log")),
        ),
        ("exited", _) => (
            RunStatus::Idle,
            "exited".into(),
            "the runner exited cleanly; a follow-up restarts it and resumes the session".into(),
        ),
        ("installing", Some(true)) => (
            RunStatus::Running,
            "installing".into(),
            "installing the harness".into(),
        ),
        ("starting", Some(true)) | ("", Some(true)) => (
            RunStatus::Running,
            "starting".into(),
            "starting the agent".into(),
        ),
        ("running", Some(true)) => (
            RunStatus::Running,
            "working".into(),
            "a turn is running".into(),
        ),
        ("idle", Some(true)) => (
            RunStatus::Idle,
            "waiting".into(),
            "waiting for a follow-up".into(),
        ),
        ("", Some(false)) => (
            RunStatus::Crashed,
            "crashed".into(),
            "the run's process is gone and it recorded no state".into(),
        ),
        (_, Some(false)) => (
            RunStatus::Crashed,
            "crashed".into(),
            format!("the run's process is gone while it was {st}"),
        ),
        (other, Some(true)) => (
            RunStatus::Running,
            other.into(),
            format!("the run is {other}"),
        ),
    }
}

impl Agents {
    /// Agent runs over `guest`. Reads the guest user's home once and finds
    /// the guest's own MCP endpoint (handed to every run unless it opts
    /// out), authenticated with `guest`'s token.
    pub async fn new(guest: SpacesdClient) -> Result<Agents> {
        let out = guest
            .run(Command::shell("printf %s \"$HOME\"").timeout(Duration::from_secs(30)))
            .await?;
        let home = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if home.is_empty() || !home.starts_with('/') {
            return Err(Error::Unsupported(
                "agent runs need a POSIX guest with a home directory".into(),
            ));
        }
        let token = guest.token().map(str::to_string);
        Ok(Agents {
            guest,
            home,
            spacesd_mcp: None,
        }
        .with_guest_mcp(token)
        .await)
    }

    /// Discovers the guest's own MCP endpoint (loopback in the guest) and
    /// hands it to every run unless a run opts out. `token` is the spacesd
    /// env token, sent as a bearer header (through `secrets.json`).
    pub async fn with_guest_mcp(mut self, token: Option<String>) -> Self {
        let path = self
            .guest
            .capabilities()
            .await
            .ok()
            .and_then(|c| c.side_channels)
            .map(|s| s.mcp_http_path)
            .filter(|p| !p.is_empty());
        if let Some(path) = path {
            let port = self
                .sh(
                    "printf %s \"${CUA_ENV_PORT:-3211}\"",
                    Duration::from_secs(30),
                )
                .await
                .ok()
                .and_then(|s| s.trim().parse::<u16>().ok())
                .unwrap_or(cua_proto::SPACESD_DEFAULT_PORT);
            self.spacesd_mcp = Some(SpacesdMcp {
                url: format!("http://127.0.0.1:{port}{path}"),
                token: token.filter(|t| !t.is_empty()),
            });
        }
        self
    }

    /// Sets the guest MCP endpoint explicitly.
    pub fn spacesd_mcp(mut self, mcp: Option<SpacesdMcp>) -> Self {
        self.spacesd_mcp = mcp;
        self
    }

    /// The guest user's home.
    pub fn home(&self) -> &str {
        &self.home
    }

    /// Guest directory of persistent agent `name`'s home.
    pub fn agent_home_dir(&self, name: &str) -> String {
        format!("{}/{HOMES_DIR}/{name}", self.home.trim_end_matches('/'))
    }

    /// Guest directory of `run_id`.
    pub fn run_dir(&self, run_id: &str) -> String {
        format!("{}/{RUNS_DIR}/{run_id}", self.home.trim_end_matches('/'))
    }

    async fn sh(&self, script: &str, timeout: Duration) -> Result<String> {
        let out = self
            .guest
            .run(Command::shell(script).timeout(timeout))
            .await?;
        if !out.status.success() {
            return Err(Error::Guest(format!(
                "guest command failed ({:?}): {}",
                out.status.code,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    async fn write(&self, path: &str, bytes: impl Into<Vec<u8>>, mode: u32) -> Result<()> {
        self.guest
            .upload(
                path,
                bytes.into(),
                UploadOptions {
                    mode: pb::WriteMode::Overwrite,
                    create_parents: true,
                    permissions: mode,
                    ..Default::default()
                },
            )
            .await?;
        Ok(())
    }

    async fn read(&self, path: &str, offset: u64, length: u64) -> Result<Option<Vec<u8>>> {
        let mut buf = vec![];
        match self
            .guest
            .download_with(
                path,
                DownloadOptions {
                    offset,
                    length,
                    ..Default::default()
                },
                &mut buf,
            )
            .await
        {
            Ok(_) => Ok(Some(buf)),
            Err(cua_spacesd_client::Error::PathNotFound(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Installs `ids` (and their dependencies) now, reporting progress.
    pub async fn ensure(
        &self,
        ids: &[&str],
        progress: impl FnMut(&installables::Progress) + Send,
    ) -> Result<Vec<installables::Progress>> {
        self.run_install(installables::script(ids)?, progress).await
    }

    /// Puts only the OS packages of `ids` in place (see
    /// [`installables::deps_script`]), for a caller that stages the
    /// archives meanwhile; [`Agents::ensure`] then finds them.
    pub async fn ensure_os_packages(
        &self,
        ids: &[&str],
        progress: impl FnMut(&installables::Progress) + Send,
    ) -> Result<Vec<installables::Progress>> {
        self.run_install(installables::deps_script(ids)?, progress)
            .await
    }

    /// The guest's spacesd client.
    pub fn guest(&self) -> &SpacesdClient {
        &self.guest
    }

    async fn run_install(
        &self,
        script: String,
        mut progress: impl FnMut(&installables::Progress) + Send,
    ) -> Result<Vec<installables::Progress>> {
        let mut h = self
            .guest
            .spawn(Command::new("/bin/sh").arg("-c").arg(script))
            .await?;
        let mut seen = vec![];
        let mut buf = String::new();
        let mut stderr = String::new();
        let mut exit = None;
        // Bounded: an install ends or the guest stream ends.
        for _ in 0..1_000_000 {
            match h.next_event().await? {
                Some(cua_spacesd_client::ProcessEvent::Stdout { data, .. }) => {
                    buf.push_str(&String::from_utf8_lossy(&data));
                    while let Some(nl) = buf.find('\n') {
                        let line: String = buf.drain(..=nl).collect();
                        if let Some(p) = installables::Progress::parse(&line) {
                            progress(&p);
                            seen.push(p);
                        }
                    }
                }
                Some(cua_spacesd_client::ProcessEvent::Stderr { data, .. }) => {
                    if stderr.len() < 8192 {
                        stderr.push_str(&String::from_utf8_lossy(&data));
                    }
                }
                Some(cua_spacesd_client::ProcessEvent::Exit(s)) => {
                    exit = Some(s);
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        match exit {
            Some(s) if s.success() => Ok(seen),
            _ => Err(Error::Install(
                seen.iter()
                    .rev()
                    .find(|p| p.phase == "error")
                    .map(|p| format!("{}: {}", p.id, p.detail))
                    .unwrap_or_else(|| format!("install failed: {}", stderr.trim())),
            )),
        }
    }

    /// Copies the cua skills into `h`'s skills directory in the guest (the
    /// same registry and bundle `cua agents setup` uses on the host).
    pub async fn install_skills(&self, h: &Harness) -> Result<Vec<String>> {
        let Some(spec) = h.skills_agent.and_then(cua_agent_setup::registry::find) else {
            return Ok(vec![]);
        };
        let Some(loc) = spec.skills else {
            return Ok(vec![]);
        };
        let env = cua_agent_setup::HostEnv {
            home: self.home.clone().into(),
            vars: Default::default(),
            path: vec![],
            app_dirs: vec![],
            os: cua_agent_setup::Os::Linux,
            run_agent_clis: false,
        };
        let dir = loc.resolve(&env).to_string_lossy().replace('\\', "/");
        let mut done = vec![];
        for skill in cua_agent_setup::skills::bundled() {
            for (rel, bytes) in cua_agent_setup::skills::files(&skill.name)? {
                self.write(
                    &format!("{dir}/{}/{rel}", skill.name),
                    bytes.to_vec(),
                    0o644,
                )
                .await?;
            }
            done.push(format!("{dir}/{}", skill.name));
        }
        Ok(done)
    }

    /// Uploads the bridge pipe once per version and returns its path.
    async fn bridge_path(&self) -> Result<String> {
        use sha2::Digest;
        let hash = hex::encode(&sha2::Sha256::digest(BRIDGE.as_bytes())[..6]);
        let path = format!(
            "{}/{}/cua-bridge/{hash}/bridge.mjs",
            self.home.trim_end_matches('/'),
            installables::TOOLS_DIR
        );
        if self.read(&path, 0, 1).await?.is_none() {
            self.write(&path, BRIDGE.as_bytes().to_vec(), 0o644).await?;
        }
        Ok(path)
    }

    async fn runner_dir(&self) -> Result<String> {
        use sha2::Digest;
        let hash = hex::encode(&sha2::Sha256::digest(RUNNER.as_bytes())[..6]);
        let dir = format!(
            "{}/{}/cua-runner/{hash}",
            self.home.trim_end_matches('/'),
            installables::TOOLS_DIR
        );
        if self
            .read(&format!("{dir}/runner.mjs"), 0, 1)
            .await?
            .is_none()
        {
            self.write(
                &format!("{dir}/runner.mjs"),
                RUNNER.as_bytes().to_vec(),
                0o644,
            )
            .await?;
        }
        Ok(dir)
    }

    #[allow(clippy::too_many_arguments)]
    fn launch_script(
        home: &str,
        h: &Harness,
        rdir: &str,
        runner_dir: &str,
        install: bool,
        repo: Option<(&str, Option<&str>, &str)>,
        prelude: &[String],
    ) -> Result<String> {
        let q = quote(rdir);
        let mut ids: Vec<&str> = vec!["acp-sdk", "mcp-remote"];
        ids.extend(h.installs.iter().copied());
        let mut s = format!(
            "#!/bin/sh\n# cua agent run launcher (generated)\numask 077\ncd {q} || exit 1\n\
             export CUA_EVENTS_FILE={q}/events.jsonl\n\
             cua_state() {{ printf '{{\"status\":\"%s\",\"error\":%s,\"turn\":0}}' \"$1\" \"$2\" > {q}/.state.tmp && mv {q}/.state.tmp {q}/state.json; }}\n\
             cua_event() {{ n=$(( $(wc -l < {q}/events.jsonl 2>/dev/null || echo 0) + 1 )); \
             printf '{{\"seq\":%s,\"ts\":%s000,\"turn\":0,\"type\":\"%s\",\"message\":\"%s\"}}\\n' \"$n\" \"$(date +%s)\" \"$1\" \"$2\" >> {q}/events.jsonl; }}\n\
             [ -f {q}/state.json ] || cua_state installing null\n"
        );
        if install {
            s.push_str(&format!(
                "(\n{}\n) >> {q}/install.log 2>&1 || {{ cua_state failed '\"install failed; see install.log\"'; cua_event error 'install failed; see install.log'; exit 1; }}\n",
                installables::script(&ids)?
            ));
        } else {
            s.push_str("export PATH=\"$HOME/.cua/bin:$PATH\"\n");
        }
        if let Some((url, branch, cwd)) = repo {
            let branch = branch
                .map(|b| format!("--branch {} ", quote(b)))
                .unwrap_or_default();
            s.push_str(&format!(
                "if [ ! -e {cwd}/.git ]; then\n\
                 cua_event notice {msg}\n\
                 git clone --quiet --depth 50 {branch}{url} {cwd} >> {q}/install.log 2>&1 \
                 || {{ cua_state failed '\"git clone failed; see install.log\"'; cua_event error 'git clone failed'; exit 1; }}\n\
                 fi\n",
                cwd = quote(cwd),
                url = quote(url),
                msg = quote(&format!("cloning {url}")),
            ));
        }
        for line in prelude {
            s.push_str(line);
            s.push('\n');
        }
        let acp_sdk = installables::install_dir(home, "acp-sdk").expect("in manifest");
        s.push_str(&format!(
            "ln -sfn {} {}/node_modules\n\
             exec \"$HOME/.cua/bin/node\" {}/runner.mjs {q}\n",
            quote(&format!("{acp_sdk}/node_modules")),
            quote(runner_dir),
            quote(runner_dir),
        ));
        Ok(s)
    }

    async fn spawn_run(&self, run_id: &str) -> Result<String> {
        let tag = format!("{TAG_PREFIX}{run_id}");
        let rdir = self.run_dir(run_id);
        let handle = self
            .guest
            .spawn(
                Command::new("/bin/sh")
                    .arg(format!("{rdir}/launch.sh"))
                    .cwd(rdir)
                    .tag(tag.clone()),
            )
            .await?;
        handle.detach();
        Ok(tag)
    }

    /// Starts `harness` on `prompt` and returns at once: the run continues
    /// in the sandbox (fire and forget), readable through
    /// [`Agents::events`], [`Agents::status`] and [`Agents::result`].
    pub async fn start(&self, harness: &str, prompt: &str, opts: RunOptions) -> Result<Started> {
        let h = harness::harness(harness).ok_or_else(|| {
            Error::Invalid(format!(
                "unknown harness {harness:?}; known: {}",
                harness::ids().join(", ")
            ))
        })?;
        if !h.ready {
            return Err(Error::Unsupported(format!(
                "{} is not supported yet (ready: {})",
                h.name,
                harness::ready().join(", ")
            )));
        }
        for k in opts.env.keys() {
            valid_env_name(k)?;
        }
        let run_id = format!("run-{:08x}", rand::random::<u32>());
        let rdir = self.run_dir(&run_id);
        if let Some(name) = &opts.home
            && !valid_home_name(name)
        {
            return Err(Error::Invalid(format!(
                "agent name {name:?}: use 1-63 of a-z, 0-9, `.`, `_`, `-`"
            )));
        }
        if opts.home_dir.is_some() && opts.home.is_none() {
            return Err(Error::Invalid(
                "home_dir needs home (the agent's name)".into(),
            ));
        }
        if let Some(d) = &opts.home_dir
            && !d.starts_with('/')
        {
            return Err(Error::Invalid(format!("home_dir {d:?} must be absolute")));
        }
        let home_dir = opts.home.as_deref().map(|n| {
            opts.home_dir
                .clone()
                .unwrap_or_else(|| self.agent_home_dir(n))
        });
        let cwd = opts.cwd.clone().unwrap_or_else(|| match &home_dir {
            Some(h) => format!("{h}/work"),
            None => format!("{rdir}/work"),
        });
        let mut notes = vec![];
        // The run directory is private to the guest user. With a repo, the
        // clone creates the working directory.
        let mk_cwd = if opts.repo.is_some() {
            String::new()
        } else {
            quote(&cwd)
        };
        let mk_home = home_dir.as_deref().map(quote).unwrap_or_default();
        self.sh(
            &format!(
                "umask 077; mkdir -p {mk_cwd} {mk_home} {r}/inbox/done {r}/attachments {r}/{BRIDGE_DIR}",
                r = quote(&rdir)
            ),
            Duration::from_secs(30),
        )
        .await?;
        let endpoint = opts.endpoint.clone().map(|mut e| {
            if e.model.is_none() {
                e.model = opts.model.clone();
            }
            e
        });
        let launch = harness::launch_with_home(
            h,
            endpoint.as_ref(),
            opts.model.as_deref(),
            &rdir,
            &cwd,
            home_dir.as_deref(),
        )?;
        for f in &launch.files {
            let path = if f.rel.starts_with('/') {
                f.rel.clone()
            } else {
                format!("{rdir}/{}", f.rel)
            };
            self.write(&path, f.body.clone(), 0o600).await?;
        }
        // Secrets: the caller's env, plus MCP header values.
        let mut secrets: BTreeMap<String, String> = opts.env.clone();
        let mut mcp: Vec<Value> = vec![];
        let mut servers = opts.mcp_servers.clone();
        if opts.sandbox_mcp.unwrap_or(h.sandbox_mcp)
            && let Some(g) = &self.spacesd_mcp
        {
            servers.insert(0, g.server_for(&run_id));
        }
        if opts.bridge {
            let bridge = self.bridge_path().await?;
            servers.push(McpServer {
                name: BRIDGE_SERVER.into(),
                command: Some(format!("{}/.cua/bin/node", self.home.trim_end_matches('/'))),
                args: vec![bridge, format!("{rdir}/{BRIDGE_DIR}")],
                ..Default::default()
            });
        }
        for (i, s) in servers.iter().enumerate() {
            if let Some(url) = &s.url {
                let headers: Vec<Value> = s
                    .headers
                    .iter()
                    .enumerate()
                    .map(|(j, (k, v))| {
                        let name = format!("CUA_MCP_{i}_{j}");
                        secrets.insert(name.clone(), v.clone());
                        json!({"name": k, "value": format!("${{{name}}}")})
                    })
                    .collect();
                mcp.push(json!({"type": "http", "name": s.name, "url": url, "headers": headers}));
            } else if let Some(cmd) = &s.command {
                let env: Vec<Value> = s
                    .env
                    .iter()
                    .enumerate()
                    .map(|(j, (k, v))| {
                        let name = format!("CUA_MCP_{i}_E{j}");
                        secrets.insert(name.clone(), v.clone());
                        json!({"name": k, "value": format!("${{{name}}}")})
                    })
                    .collect();
                mcp.push(json!({"name": s.name, "command": cmd, "args": s.args, "env": env}));
            } else {
                return Err(Error::Invalid(format!(
                    "MCP server {:?} needs a url or a command",
                    s.name
                )));
            }
        }
        // Attachments.
        let mut files = vec![];
        for a in &opts.files {
            let name = std::path::Path::new(&a.name)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.is_empty() && n != "." && n != "..")
                .ok_or_else(|| Error::Invalid(format!("bad attachment name {:?}", a.name)))?;
            let path = format!("{rdir}/attachments/{name}");
            self.write(&path, a.bytes.clone(), 0o600).await?;
            files.push(path);
        }
        if opts.skills.unwrap_or(true) {
            match self.install_skills(h).await {
                Ok(_) => {}
                Err(e) => notes.push(format!("cua skills were not installed: {e}")),
            }
        }
        let (command, args) = match &launch.wrapper {
            Some(w) => {
                self.write(&format!("{rdir}/agent.sh"), w.clone(), 0o700)
                    .await?;
                ("/bin/sh".to_string(), vec![format!("{rdir}/agent.sh")])
            }
            None => {
                let mut args: Vec<String> = h.acp[1..].iter().map(|s| s.to_string()).collect();
                args.extend(launch.args.iter().cloned());
                (format!("{}/.cua/bin/{}", self.home, h.acp[0]), args)
            }
        };
        let mut env: BTreeMap<String, String> = launch.env.iter().cloned().collect();
        env.insert(
            "PATH".into(),
            format!(
                "{}/.cua/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                self.home
            ),
        );
        let run = json!({
            "harness": h.id,
            "agent": {"command": command, "args": args},
            "cwd": cwd,
            "env": env,
            "prompt": prompt,
            "files": files,
            "mcpServers": mcp,
            "exitWhenIdle": opts.exit_when_idle,
            "idleExitSecs": opts.idle_exit_secs.unwrap_or(1800),
            "authMethod": h.auth_method,
            "mcpBridge": format!("{}/.cua/bin/mcp-remote", self.home),
            "modeKinds": h.mode_kinds,
        });
        self.write(
            &format!("{rdir}/run.json"),
            serde_json::to_vec_pretty(&run)?,
            0o600,
        )
        .await?;
        self.write(
            &format!("{rdir}/secrets.json"),
            serde_json::to_vec(&secrets)?,
            0o600,
        )
        .await?;
        let meta = RunMeta {
            run_id: run_id.clone(),
            harness: h.id.into(),
            prompt: prompt.into(),
            cwd: cwd.clone(),
            created_at: now(),
            label: opts.label.clone(),
            repo: opts.repo.clone(),
            home: opts.home.clone(),
        };
        self.write(
            &format!("{rdir}/meta.json"),
            serde_json::to_vec(&meta)?,
            0o600,
        )
        .await?;
        let runner_dir = self.runner_dir().await?;
        let script = Self::launch_script(
            &self.home,
            h,
            &rdir,
            &runner_dir,
            opts.install.unwrap_or(true),
            opts.repo
                .as_deref()
                .map(|r| (r, opts.branch.as_deref(), cwd.as_str())),
            &launch.prelude,
        )?;
        self.write(&format!("{rdir}/launch.sh"), script, 0o700)
            .await?;
        let tag = self.spawn_run(&run_id).await?;
        Ok(Started {
            run_id,
            harness: h.id.into(),
            process_tag: tag,
            cwd,
            run_dir: rdir,
            notes,
        })
    }

    async fn alive(&self, run_id: &str) -> Option<bool> {
        let tag = format!("{TAG_PREFIX}{run_id}");
        let procs = self.guest.list_processes(false).await.ok()?;
        Some(procs.iter().any(|p| p.tag == tag))
    }

    async fn json_file(&self, path: &str) -> Result<Option<Value>> {
        Ok(self
            .read(path, 0, 0)
            .await?
            .and_then(|b| serde_json::from_slice(&b).ok()))
    }

    /// One run's status. Never disturbs the run.
    pub async fn status(&self, run_id: &str) -> Result<RunInfo> {
        valid_run_id(run_id)?;
        let rdir = self.run_dir(run_id);
        let meta: Option<RunMeta> = match self.json_file(&format!("{rdir}/meta.json")).await {
            Ok(m) => m.and_then(|v| serde_json::from_value(v).ok()),
            Err(e) => {
                return Ok(RunInfo {
                    run_id: run_id.into(),
                    harness: None,
                    status: RunStatus::Unknown,
                    phase: "unknown".into(),
                    reason: format!("could not read the run: {e}"),
                    turn: 0,
                    session_id: None,
                    stop_reason: None,
                    error: None,
                    alive: None,
                    accepts_message: false,
                    meta: None,
                });
            }
        };
        let Some(meta) = meta else {
            return Err(Error::NotFound(format!("no run {run_id} in this sandbox")));
        };
        let state = self
            .json_file(&format!("{rdir}/state.json"))
            .await
            .ok()
            .flatten()
            .unwrap_or(Value::Null);
        let alive = self.alive(run_id).await;
        Ok(info(run_id, meta, &state, alive))
    }

    /// Every run in the sandbox, newest first.
    pub async fn list(&self) -> Result<Vec<RunInfo>> {
        let root = format!("{}/{RUNS_DIR}", self.home.trim_end_matches('/'));
        let out = self
            .sh(
                &format!(
                    "for d in {q}/run-*/; do [ -f \"$d/meta.json\" ] || continue; \
                     cat \"$d/meta.json\"; printf %s {rs}; cat \"$d/state.json\" 2>/dev/null; printf %s {rs}; done; true",
                    q = quote(&root),
                    rs = quote(RS)
                ),
                Duration::from_secs(60),
            )
            .await?;
        let procs = self.guest.list_processes(false).await.ok();
        let parts: Vec<&str> = out.split(RS).collect();
        let mut runs = vec![];
        for pair in parts.chunks(2) {
            let [m, s] = pair else { continue };
            let Ok(meta) = serde_json::from_str::<RunMeta>(m.trim()) else {
                continue;
            };
            let state: Value = serde_json::from_str(s.trim()).unwrap_or(Value::Null);
            let alive = procs.as_ref().map(|p| {
                let tag = format!("{TAG_PREFIX}{}", meta.run_id);
                p.iter().any(|x| x.tag == tag)
            });
            runs.push(info(&meta.run_id.clone(), meta, &state, alive));
        }
        runs.sort_by(|a, b| {
            let t = |r: &RunInfo| r.meta.as_ref().map(|m| m.created_at).unwrap_or(0.0);
            t(b).total_cmp(&t(a))
        });
        Ok(runs)
    }

    /// Events from byte `cursor` (0 = the start), at most `max`.
    pub async fn events(&self, run_id: &str, cursor: u64, max: usize) -> Result<EventPage> {
        valid_run_id(run_id)?;
        let path = format!("{}/events.jsonl", self.run_dir(run_id));
        let bytes = self
            .read(&path, cursor, PAGE_BYTES)
            .await?
            .unwrap_or_default();
        Ok(events::page(&bytes, cursor, max.max(1)))
    }

    /// Every event (bounded to the last 8 MiB).
    async fn all_events(&self, run_id: &str) -> Result<Vec<AgentEvent>> {
        let path = format!("{}/events.jsonl", self.run_dir(run_id));
        let bytes = self
            .read(&path, 0, 8 * 1024 * 1024)
            .await?
            .unwrap_or_default();
        Ok(events::page(&bytes, 0, usize::MAX).events)
    }

    async fn post(&self, run_id: &str, msg: Value) -> Result<()> {
        let name = format!(
            "{:013}-{:04x}.json",
            (now() * 1000.0) as u64,
            rand::random::<u16>()
        );
        self.write(
            &format!("{}/inbox/{name}", self.run_dir(run_id)),
            serde_json::to_vec(&msg)?,
            0o600,
        )
        .await
    }

    /// Sends a follow-up. A running turn finishes first (the prompt is
    /// queued); a runner that exited is restarted and resumes the session.
    pub async fn send(&self, run_id: &str, text: &str, files: Vec<Attachment>) -> Result<RunInfo> {
        let before = self.status(run_id).await?;
        if matches!(before.status, RunStatus::Unknown) {
            return Err(Error::Guest(before.reason));
        }
        let rdir = self.run_dir(run_id);
        let mut paths = vec![];
        for a in &files {
            let name = std::path::Path::new(&a.name)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .ok_or_else(|| Error::Invalid(format!("bad attachment name {:?}", a.name)))?;
            let p = format!("{rdir}/attachments/{name}");
            self.write(&p, a.bytes.clone(), 0o600).await?;
            paths.push(p);
        }
        self.post(
            run_id,
            json!({"op": "prompt", "text": text, "files": paths}),
        )
        .await?;
        if before.alive == Some(false) {
            // Restart: installs are cached, the runner resumes the session.
            if before.status == RunStatus::Failed {
                self.write(
                    &format!("{rdir}/state.json"),
                    br#"{"status":"starting","error":null}"#.to_vec(),
                    0o600,
                )
                .await?;
            }
            self.spawn_run(run_id).await?;
        }
        self.status(run_id).await
    }

    /// Interrupts the turn in flight (ACP `session/cancel`); the session
    /// stays open for a follow-up.
    pub async fn interrupt(&self, run_id: &str) -> Result<RunInfo> {
        let s = self.status(run_id).await?;
        if s.alive == Some(true) {
            self.post(run_id, json!({"op": "cancel"})).await?;
        }
        Ok(s)
    }

    /// Stops the run and verifies its process is gone.
    pub async fn stop(&self, run_id: &str) -> Result<RunInfo> {
        let s = self.status(run_id).await?;
        // A terminal following the run (`show`), if any.
        let _ = self
            .guest
            .process()
            .signal_process(pb::SignalProcessRequest {
                process: Some(pb::ProcessSelector {
                    selector: Some(pb::process_selector::Selector::Tag(format!(
                        "{TAG_PREFIX}{run_id}/view"
                    ))),
                }),
                signal: pb::Signal::Term as i32,
                process_group: true,
            })
            .await;
        if s.alive == Some(true) {
            self.post(run_id, json!({"op": "stop"})).await?;
            for _ in 0..30 {
                if self.alive(run_id).await == Some(false) {
                    return self.status(run_id).await;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            for sig in [pb::Signal::Term, pb::Signal::Kill] {
                let _ = self
                    .guest
                    .process()
                    .signal_process(pb::SignalProcessRequest {
                        process: Some(pb::ProcessSelector {
                            selector: Some(pb::process_selector::Selector::Tag(format!(
                                "{TAG_PREFIX}{run_id}"
                            ))),
                        }),
                        signal: sig as i32,
                        process_group: true,
                    })
                    .await;
                tokio::time::sleep(Duration::from_secs(2)).await;
                if self.alive(run_id).await == Some(false) {
                    break;
                }
            }
        }
        self.status(run_id).await
    }

    /// The last turn's outcome, from the event log.
    pub async fn result(&self, run_id: &str) -> Result<RunResult> {
        let s = self.status(run_id).await?;
        let evs = self.all_events(run_id).await?;
        Ok(summarize(run_id, s.status, &evs))
    }

    /// Waits until no turn is running (or `timeout`), then returns the result.
    pub async fn wait(&self, run_id: &str, timeout: Duration) -> Result<RunResult> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let s = self.status(run_id).await?;
            if s.status != RunStatus::Running {
                return self.result(run_id).await;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "{run_id} is still {} after {timeout:?}",
                    s.phase
                )));
            }
            tokio::time::sleep(Duration::from_millis(750)).await;
        }
    }

    /// Files in the run's working directory changed since it started.
    pub async fn artifacts(&self, run_id: &str) -> Result<Vec<Artifact>> {
        let s = self.status(run_id).await?;
        let meta = s
            .meta
            .ok_or_else(|| Error::NotFound(format!("no run {run_id}")))?;
        let out = self
            .sh(
                &format!(
                    "find {cwd} -type f -newer {rdir}/meta.json -not -path '*/.git/*' -not -path '*/node_modules/*' \
                     -printf '%s\\t%T@\\t%p\\n' 2>/dev/null | head -n 2000; true",
                    cwd = quote(&meta.cwd),
                    rdir = quote(&self.run_dir(run_id)),
                ),
                Duration::from_secs(60),
            )
            .await?;
        Ok(out
            .lines()
            .filter_map(|l| {
                let mut p = l.splitn(3, '\t');
                let size = p.next()?.parse().ok()?;
                let t: f64 = p.next()?.parse().ok()?;
                Some(Artifact {
                    size,
                    modified_ms: (t * 1000.0) as u64,
                    path: p.next()?.to_string(),
                })
            })
            .collect())
    }

    /// Stops the run and deletes its directory (secrets included).
    pub async fn remove(&self, run_id: &str) -> Result<()> {
        let s = self.status(run_id).await?;
        if s.alive == Some(true) {
            self.stop(run_id).await?;
        }
        self.guest.remove(&self.run_dir(run_id), true).await?;
        Ok(())
    }
}

fn info(run_id: &str, meta: RunMeta, state: &Value, alive: Option<bool>) -> RunInfo {
    let (status, phase, reason) = classify(state, alive);
    RunInfo {
        run_id: run_id.into(),
        harness: Some(meta.harness.clone()),
        status,
        phase,
        reason,
        turn: state.get("turn").and_then(Value::as_u64).unwrap_or(0) as u32,
        session_id: state
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_string),
        stop_reason: state
            .get("stopReason")
            .and_then(Value::as_str)
            .map(str::to_string),
        error: state
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_string),
        alive,
        accepts_message: accepts_message(status),
        meta: Some(meta),
    }
}

/// The last turn's outcome from its events.
pub fn summarize(run_id: &str, status: RunStatus, evs: &[AgentEvent]) -> RunResult {
    let turn = evs.iter().map(|e| e.turn).max().unwrap_or(0);
    let last: Vec<&AgentEvent> = evs.iter().filter(|e| e.turn == turn).collect();
    let text = last
        .iter()
        .filter(|e| e.kind == "message")
        .filter_map(|e| e.text.as_deref())
        .collect::<Vec<_>>()
        .join("");
    let ended = last.iter().rev().find(|e| e.kind == "turn_ended");
    RunResult {
        run_id: run_id.into(),
        status,
        turn,
        text,
        stop_reason: ended.and_then(|e| e.stop_reason.clone()),
        usage: ended
            .and_then(|e| e.raw.get("usage").cloned())
            .filter(|u| !u.is_null()),
        error: evs
            .iter()
            .rev()
            .find(|e| e.kind == "error")
            .and_then(|e| e.text.clone()),
        tool_calls: last.iter().filter(|e| e.kind == "tool_call").count() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every MCP session of one run names the run, so the guest draws one
    /// cursor for it (cua-spacesd `/mcp` keys the driver session by it).
    #[test]
    fn the_guest_mcp_names_the_run() {
        let g = SpacesdMcp {
            url: "http://127.0.0.1:3211/mcp".into(),
            token: Some("t0ken".into()),
        };
        let s = g.server_for("run-84a8dc1f");
        assert_eq!(s.name, "cua-driver");
        assert_eq!(s.url.as_deref(), Some("http://127.0.0.1:3211/mcp"));
        assert_eq!(s.headers["Authorization"], "Bearer t0ken");
        assert_eq!(s.headers[AGENT_SESSION_HEADER], "run-84a8dc1f");
        let anonymous = SpacesdMcp { token: None, ..g };
        assert_eq!(
            anonymous
                .server_for("run-1")
                .headers
                .keys()
                .collect::<Vec<_>>(),
            vec![AGENT_SESSION_HEADER]
        );
    }

    #[test]
    fn run_ids_and_env_names_are_validated_before_they_reach_a_shell() {
        assert!(valid_run_id("run-84a8dc1f").is_ok());
        for bad in ["", "../x", "a b", "run;rm", &"x".repeat(65)] {
            assert!(valid_run_id(bad).is_err(), "{bad}");
        }
        assert!(valid_env_name("ANTHROPIC_API_KEY").is_ok());
        for bad in ["", "A-B", "1X", "A B", "X;rm"] {
            assert!(valid_env_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_status_ladder() {
        let s = |st: &str| json!({"status": st});
        assert_eq!(classify(&s("running"), Some(true)).0, RunStatus::Running);
        assert_eq!(classify(&s("installing"), Some(true)).1, "installing");
        assert_eq!(classify(&s("idle"), Some(true)).0, RunStatus::Idle);
        assert_eq!(classify(&s("exited"), Some(false)).0, RunStatus::Idle);
        assert_eq!(classify(&s("failed"), Some(false)).0, RunStatus::Failed);
        assert_eq!(classify(&s("running"), Some(false)).0, RunStatus::Crashed);
        assert_eq!(classify(&Value::Null, Some(false)).0, RunStatus::Crashed);
        assert_eq!(classify(&s("running"), None).0, RunStatus::Unknown);
    }

    #[test]
    fn a_follow_up_is_accepted_only_when_it_starts_the_next_turn() {
        let accepts = |st: &str, alive| {
            let (status, _, _) = classify(&json!({"status": st}), alive);
            accepts_message(status)
        };
        assert!(accepts("idle", Some(true)), "waiting for a follow-up");
        assert!(accepts("exited", Some(false)), "resumes the session");
        assert!(accepts("running", Some(false)), "crashed: restarts");
        assert!(accepts("failed", Some(false)), "failed: send restarts it");
        for st in ["running", "installing", "starting", ""] {
            assert!(!accepts(st, Some(true)), "{st:?}: a turn is in flight");
        }
        assert!(!accepts("idle", None), "unknown: could not be read");
    }

    #[test]
    fn a_recorded_run_summarizes_to_its_final_message() {
        let raw = include_str!("../tests/fixtures/recorded/claude-agent-acp-0.81.1.events.jsonl");
        let evs = events::page(raw.as_bytes(), 0, usize::MAX).events;
        let r = summarize("run-1", RunStatus::Idle, &evs);
        assert_eq!(r.turn, 1);
        assert!(
            r.text.contains("Done. Last tool output: hello-from-mock"),
            "{}",
            r.text
        );
        assert_eq!(r.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(r.tool_calls, 1);
        assert!(r.usage.is_some());
    }

    /// The launcher is valid shell and never embeds a secret.
    #[test]
    fn launch_scripts_parse() {
        for h in harness::HARNESSES {
            let s = Agents::launch_script(
                "/root",
                h,
                "/root/.cua/agents/run-1",
                "/root/.cua/tools/cua-runner/x",
                true,
                Some(("https://github.com/o/r", Some("main"), "/w")),
                &["[ -e \"$HOME/x\" ] && ln -sf \"$HOME/x\" /r/y; true".to_string()],
            )
            .unwrap();
            let out = std::process::Command::new("/bin/sh")
                .arg("-n")
                .arg("-c")
                .arg(&s)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}: {}",
                h.id,
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
