//! Coding agents in any sandbox (`sandbox.agents()`, `guest.agents()`):
//! the `cua-agents` runner over the sandbox's cua-spacesd.
//!
//! ```python
//! agents = await sb.agents()
//! run = await agents.run("claude-code", "fix the failing test",
//!                        cua.AgentRunOptions(env={"ANTHROPIC_API_KEY": key}, repo=url))
//! async for e in run.stream():          # Python helper over run.events(cursor)
//!     print(e.kind, e.text)
//! await run.send("now add a regression test")
//! print((await run.result()).text)
//! ```

use super::{SpacesdClient, run};
use crate::{CuaError, Result};
use cua_agents as ca;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

impl From<ca::Error> for CuaError {
    fn from(e: ca::Error) -> Self {
        match e {
            ca::Error::Invalid(m) => CuaError::InvalidArgument(m),
            ca::Error::NotFound(m) => CuaError::NotFound(m),
            ca::Error::Unsupported(m) => CuaError::Unsupported(m),
            ca::Error::Timeout(m) => CuaError::Timeout(m),
            ca::Error::Client(c) => c.into(),
            other => CuaError::Env(other.to_string()),
        }
    }
}

/// An MCP server a run's agent gets: `url` (streamable HTTP, as reachable
/// from inside the sandbox) or `command` (stdio, run in the sandbox).
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct AgentRunMcpServer {
    /// Name the agent sees.
    pub name: String,
    #[uniffi(default = None)]
    pub url: Option<String>,
    /// Header values (secrets allowed: they travel like env keys).
    #[uniffi(default)]
    pub headers: HashMap<String, String>,
    #[uniffi(default = None)]
    pub command: Option<String>,
    #[uniffi(default = [])]
    pub args: Vec<String>,
    #[uniffi(default)]
    pub env: HashMap<String, String>,
}

/// A file for the first prompt or a follow-up.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentFile {
    /// File name in the run's `attachments/`.
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Options for [`Agents::run`]. Every field is optional.
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Record)]
pub struct AgentRunOptions {
    /// Working directory in the sandbox (default: the run's own).
    #[uniffi(default = None)]
    pub cwd: Option<String>,
    /// Git URL cloned into the working directory first.
    #[uniffi(default = None)]
    pub repo: Option<String>,
    #[uniffi(default = None)]
    pub branch: Option<String>,
    /// Env for the agent only (API keys): written 0600 in the run,
    /// redacted from its event log, never in argv.
    #[uniffi(default)]
    pub env: HashMap<String, String>,
    /// Provider key variables copied from THIS process's environment
    /// (`["ANTHROPIC_API_KEY"]`); only known provider key names.
    #[uniffi(default = [])]
    pub env_from_host: Vec<String>,
    /// Model id.
    #[uniffi(default = None)]
    pub model: Option<String>,
    /// A custom model endpoint (proxy, gateway, compatible server).
    #[uniffi(default = None)]
    pub base_url: Option<String>,
    /// Its wire format: `anthropic`, `openai-responses`, `openai-chat`,
    /// `gemini` (default: the harness's own).
    #[uniffi(default = None)]
    pub wire: Option<String>,
    #[uniffi(default = [])]
    pub mcp_servers: Vec<AgentRunMcpServer>,
    /// Give the agent the sandbox's own MCP (cua-driver). Default true.
    #[uniffi(default = None)]
    pub sandbox_mcp: Option<bool>,
    /// Copy the cua skills into the harness. Default true.
    #[uniffi(default = None)]
    pub skills: Option<bool>,
    /// Install what the harness needs. Default true.
    #[uniffi(default = None)]
    pub install: Option<bool>,
    #[uniffi(default = [])]
    pub files: Vec<AgentFile>,
    /// Stop (resumably) once the queue is empty: fire-and-forget.
    #[uniffi(default = false)]
    pub exit_when_idle: bool,
    #[uniffi(default = None)]
    pub label: Option<String>,
}

/// One normalized event.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentEvent {
    pub seq: u64,
    pub ts_ms: u64,
    pub turn: u32,
    /// `message`, `thought`, `tool_call`, `tool_update`, `plan`, `usage`,
    /// `permission`, `turn_started`, `turn_ended`, `install`, `error`,
    /// `exited`, ... (see `harnesses()`).
    pub kind: String,
    pub text: Option<String>,
    pub tool_id: Option<String>,
    pub tool_title: Option<String>,
    pub tool_kind: Option<String>,
    pub tool_status: Option<String>,
    pub stop_reason: Option<String>,
    /// How a conversation view shows it: `message` (the agent's words),
    /// `user` (the prompt as the agent got it), `activity` (a muted one-line
    /// row: install, thinking, tools, plan, turn end, notice, error, exit)
    /// or `hidden`. See [`AgentTranscript`].
    pub category: String,
    /// One short line for an `activity` event.
    pub summary: Option<String>,
    /// One human-readable line, when the kind has one.
    pub line: Option<String>,
    /// The event as written (ACP payload included), JSON.
    pub json: String,
}

impl From<&ca::AgentEvent> for AgentEvent {
    fn from(e: &ca::AgentEvent) -> Self {
        AgentEvent {
            seq: e.seq,
            ts_ms: e.ts_ms,
            turn: e.turn,
            kind: e.kind.into(),
            text: e.text.clone(),
            tool_id: e.tool_id.clone(),
            tool_title: e.tool_title.clone(),
            tool_kind: e.tool_kind.clone(),
            tool_status: e.tool_status.clone(),
            stop_reason: e.stop_reason.clone(),
            category: e.category.into(),
            summary: e.summary.clone(),
            line: e.render(),
            json: e.raw.to_string(),
        }
    }
}

/// The category of an event kind: `message`, `user`, `activity` or
/// `hidden` (the rule [`AgentEvent::category`] and [`AgentTranscript`] use).
#[uniffi::export]
pub fn agent_event_category(kind: String) -> String {
    ca::events::category(&kind).into()
}

/// One item of an [`AgentTranscript`].
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentTranscriptItem {
    /// `message` (a bubble or prose), `user` (the prompt as the agent got
    /// it; skip it when the app shows what the user typed) or `activity`
    /// (a muted, collapsible group of one-line steps).
    pub kind: String,
    /// Turn number (0 before the first prompt).
    pub turn: u32,
    /// The message or prompt; for `activity`, the group summary (`5 steps`).
    pub text: String,
    /// `activity` only: one line per step.
    pub steps: Vec<String>,
}

impl From<&ca::TranscriptItem> for AgentTranscriptItem {
    fn from(i: &ca::TranscriptItem) -> Self {
        AgentTranscriptItem {
            kind: i.kind.into(),
            turn: i.turn,
            text: i.text.clone(),
            steps: i.steps.clone(),
        }
    }
}

/// A run's events folded into conversation items, the same rule in every
/// language: the agent's message chunks join into `message` items, the
/// prompt is a `user` item (never repeated as agent text), and consecutive
/// activity of one turn folds into one `activity` group. Absorbing an event
/// twice changes nothing, so a poll can re-read a page.
#[derive(uniffi::Object)]
pub struct AgentTranscript {
    inner: std::sync::Mutex<(ca::Transcript, u64)>,
}

#[uniffi::export]
impl AgentTranscript {
    /// An empty transcript.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(AgentTranscript {
            inner: Default::default(),
        })
    }

    /// Adds events from [`AgentRun::events`].
    pub fn absorb(&self, events: Vec<AgentEvent>) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        for e in events {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&e.json)
                && let Some(parsed) = ca::AgentEvent::from_value(&v)
            {
                g.0.absorb(&parsed);
            }
        }
    }

    /// Adds the events of an `agent_events` result (`Space.agent_events`,
    /// the MCP tool), or a JSON array of events. Returns the page's
    /// `cursor` (also kept, see [`AgentTranscript::cursor`]) when it has one.
    pub fn absorb_json(&self, json: String) -> Result<Option<u64>> {
        let v: serde_json::Value = serde_json::from_str(&json)?;
        let (events, cursor) = match &v {
            serde_json::Value::Array(a) => (a.as_slice(), None),
            serde_json::Value::Object(o) => (
                o.get("events")
                    .and_then(|e| e.as_array())
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                o.get("cursor").and_then(|c| c.as_u64()),
            ),
            _ => {
                return Err(CuaError::InvalidArgument(
                    "absorb_json takes an agent_events result or an array of events".into(),
                ));
            }
        };
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        for e in events.iter().filter_map(ca::AgentEvent::from_value) {
            g.0.absorb(&e);
        }
        if let Some(c) = cursor {
            g.1 = g.1.max(c);
        }
        Ok(cursor)
    }

    /// Adds a line the app itself produced (a start note, "queued") as an
    /// activity step of `turn`.
    pub fn note(&self, turn: u32, text: String) {
        let mut g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.0.note(turn, &text);
    }

    /// The items so far.
    pub fn items(&self) -> Vec<AgentTranscriptItem> {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.0.items().iter().map(Into::into).collect()
    }

    /// The agent's last message on one line (a roster preview), `None`
    /// before it has said anything. Activity is never a preview.
    pub fn preview(&self) -> Option<String> {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.0.preview()
    }

    /// Changes whenever the items do.
    pub fn revision(&self) -> u64 {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.0.revision()
    }

    /// The highest `agent_events` cursor absorbed: pass it back to read on.
    pub fn cursor(&self) -> u64 {
        let g = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        g.1
    }
}

/// A page of events.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentEventPage {
    pub events: Vec<AgentEvent>,
    /// Pass back to continue.
    pub cursor: u64,
    /// Nothing more is written yet.
    pub caught_up: bool,
}

/// A run's state.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AgentRunInfo {
    pub run_id: String,
    pub harness: Option<String>,
    /// `running`, `idle`, `failed`, `crashed`, `unknown`.
    pub status: String,
    /// `installing`, `starting`, `working`, `waiting`, `exited`, ...
    pub phase: String,
    pub reason: String,
    pub turn: u32,
    pub alive: Option<bool>,
    /// A follow-up sent now starts the next turn (not mid-turn).
    pub accepts_message: bool,
    pub prompt: Option<String>,
    pub label: Option<String>,
    /// Unix seconds.
    pub created_at: Option<f64>,
    pub json: String,
}

impl From<ca::RunInfo> for AgentRunInfo {
    fn from(r: ca::RunInfo) -> Self {
        AgentRunInfo {
            json: serde_json::to_string(&r).unwrap_or_default(),
            run_id: r.run_id,
            harness: r.harness,
            status: r.status.as_str().into(),
            phase: r.phase,
            reason: r.reason,
            turn: r.turn,
            alive: r.alive,
            accepts_message: r.accepts_message,
            prompt: r.meta.as_ref().map(|m| m.prompt.clone()),
            label: r.meta.as_ref().and_then(|m| m.label.clone()),
            created_at: r.meta.as_ref().map(|m| m.created_at),
        }
    }
}

/// What the last turn produced.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AgentRunResult {
    pub run_id: String,
    pub status: String,
    pub turn: u32,
    /// The agent's messages in the last turn.
    pub text: String,
    pub stop_reason: Option<String>,
    pub usage_json: Option<String>,
    pub error: Option<String>,
    pub tool_calls: u32,
}

impl From<ca::RunResult> for AgentRunResult {
    fn from(r: ca::RunResult) -> Self {
        AgentRunResult {
            run_id: r.run_id,
            status: r.status.as_str().into(),
            turn: r.turn,
            text: r.text,
            stop_reason: r.stop_reason,
            usage_json: r.usage.map(|u| u.to_string()),
            error: r.error,
            tool_calls: r.tool_calls,
        }
    }
}

/// A file the run created or changed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AgentArtifact {
    pub path: String,
    pub size: u64,
    pub modified_ms: u64,
}

/// Agent runs in one sandbox.
#[derive(uniffi::Object)]
pub struct Agents {
    inner: ca::Agents,
}

impl Agents {
    pub(crate) async fn over(guest: cua_spacesd_client::SpacesdClient) -> Result<Arc<Agents>> {
        let inner = run(async move { Ok(ca::Agents::new(guest).await?) }).await?;
        Ok(Arc::new(Agents { inner }))
    }
}

/// The harnesses, their readiness, installs, key variables and limits, as
/// JSON.
#[uniffi::export]
pub fn agent_harnesses() -> String {
    let v: Vec<_> = ca::harness::HARNESSES.iter().map(|h| h.info()).collect();
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

fn attachments(files: Vec<AgentFile>) -> Vec<ca::Attachment> {
    files
        .into_iter()
        .map(|f| ca::Attachment {
            name: f.name,
            bytes: f.bytes,
        })
        .collect()
}

fn run_options(o: AgentRunOptions) -> Result<ca::RunOptions> {
    let mut env: std::collections::BTreeMap<String, String> = o.env.into_iter().collect();
    if !o.env_from_host.is_empty() {
        let allowed: Vec<&str> = ca::harness::HARNESSES
            .iter()
            .flat_map(|h| h.keys.iter().copied())
            .chain(["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"])
            .collect();
        for n in o.env_from_host {
            if !allowed.contains(&n.as_str()) {
                return Err(CuaError::InvalidArgument(format!(
                    "{n} is not a provider key variable"
                )));
            }
            let v = std::env::var(&n)
                .ok()
                .filter(|v| !v.is_empty())
                .ok_or_else(|| CuaError::InvalidArgument(format!("{n} is not set")))?;
            env.insert(n, v);
        }
    }
    let wire = match o.wire.as_deref() {
        None => None,
        Some(w) => Some(
            ca::Wire::parse(w)
                .ok_or_else(|| CuaError::InvalidArgument(format!("unknown wire {w:?}")))?,
        ),
    };
    Ok(ca::RunOptions {
        cwd: o.cwd,
        repo: o.repo,
        branch: o.branch,
        files: attachments(o.files),
        env,
        mcp_servers: o
            .mcp_servers
            .into_iter()
            .map(|m| ca::McpServer {
                name: m.name,
                url: m.url,
                headers: m.headers.into_iter().collect(),
                command: m.command,
                args: m.args,
                env: m.env.into_iter().collect(),
            })
            .collect(),
        sandbox_mcp: o.sandbox_mcp,
        skills: o.skills,
        endpoint: o.base_url.map(|base_url| ca::Endpoint {
            base_url,
            wire,
            model: o.model.clone(),
        }),
        model: o.model,
        install: o.install,
        exit_when_idle: o.exit_when_idle,
        idle_exit_secs: None,
        label: o.label,
        // A persistent home and the host bridge need the Spaces runtime
        // (`Space.agent_start` with `home`), which moves the home in and out.
        home: None,
        home_dir: None,
        bridge: false,
    })
}

#[uniffi::export]
impl Agents {
    /// Starts `harness` on `prompt` and returns at once; the run lives in
    /// the sandbox (fire and forget) until it is stopped.
    pub async fn run(
        &self,
        harness: String,
        prompt: String,
        options: Option<AgentRunOptions>,
    ) -> Result<Arc<AgentRun>> {
        let inner = self.inner.clone();
        let opts = run_options(options.unwrap_or_default())?;
        let started = run(async move { Ok(inner.start(&harness, &prompt, opts).await?) }).await?;
        Ok(Arc::new(AgentRun {
            inner: self.inner.clone(),
            run_id: started.run_id,
            harness: started.harness,
            started: std::time::Instant::now(),
            recorded: Default::default(),
        }))
    }

    /// A handle for an existing run (started by any client).
    pub async fn get(&self, run_id: String) -> Result<Arc<AgentRun>> {
        let inner = self.inner.clone();
        let id = run_id.clone();
        let info = run(async move { Ok(inner.status(&id).await?) }).await?;
        Ok(Arc::new(AgentRun {
            inner: self.inner.clone(),
            run_id,
            harness: info.harness.unwrap_or_default(),
            started: std::time::Instant::now(),
            recorded: Default::default(),
        }))
    }

    /// Every run in the sandbox, newest first.
    pub async fn list(&self) -> Result<Vec<AgentRunInfo>> {
        let inner = self.inner.clone();
        run(async move { Ok(inner.list().await?.into_iter().map(Into::into).collect()) }).await
    }

    /// Installs harnesses or apps (`["claude-code", "blender"]`: harness
    /// ids or installable ids) now; returns the progress lines.
    pub async fn ensure(&self, ids: Vec<String>) -> Result<Vec<String>> {
        let inner = self.inner.clone();
        run(async move {
            let mut items: Vec<&str> = vec![];
            for id in &ids {
                match ca::harness::harness(id) {
                    Some(h) => items.extend(h.installs.iter().copied()),
                    None => items.push(id),
                }
            }
            let progress = inner.ensure(&items, |_| {}).await?;
            Ok(progress
                .into_iter()
                .map(|p| format!("{} {} {}", p.id, p.phase, p.detail))
                .collect())
        })
        .await
    }
}

/// One run.
#[derive(uniffi::Object)]
pub struct AgentRun {
    inner: ca::Agents,
    run_id: String,
    harness: String,
    /// When this handle was made (the duration bucket of
    /// `cua_agent_run_completed`).
    started: std::time::Instant,
    /// `cua_agent_run_completed` was recorded for this handle.
    recorded: std::sync::atomic::AtomicBool,
}

#[uniffi::export]
impl AgentRun {
    pub fn run_id(&self) -> String {
        self.run_id.clone()
    }

    pub fn harness(&self) -> String {
        self.harness.clone()
    }

    pub async fn status(&self) -> Result<AgentRunInfo> {
        let (a, id) = (self.inner.clone(), self.run_id.clone());
        run(async move { Ok(a.status(&id).await?.into()) }).await
    }

    /// Events after `cursor` (0: from the start), at most `max`.
    pub async fn events(&self, cursor: u64, max: Option<u32>) -> Result<AgentEventPage> {
        let (a, id) = (self.inner.clone(), self.run_id.clone());
        run(async move {
            let p = a.events(&id, cursor, max.unwrap_or(200) as usize).await?;
            Ok(AgentEventPage {
                events: p.events.iter().map(Into::into).collect(),
                cursor: p.cursor,
                caught_up: p.caught_up,
            })
        })
        .await
    }

    /// A follow-up in the same session (queued while a turn runs).
    pub async fn send(&self, text: String, files: Option<Vec<AgentFile>>) -> Result<AgentRunInfo> {
        let (a, id) = (self.inner.clone(), self.run_id.clone());
        let files = attachments(files.unwrap_or_default());
        run(async move { Ok(a.send(&id, &text, files).await?.into()) }).await
    }

    /// Cancels the turn in flight; the session stays open.
    pub async fn interrupt(&self) -> Result<AgentRunInfo> {
        let (a, id) = (self.inner.clone(), self.run_id.clone());
        run(async move { Ok(a.interrupt(&id).await?.into()) }).await
    }

    /// Stops the run and verifies its process is gone.
    pub async fn stop(&self) -> Result<AgentRunInfo> {
        let (a, id) = (self.inner.clone(), self.run_id.clone());
        run(async move { Ok(a.stop(&id).await?.into()) }).await
    }

    /// The last turn's outcome.
    pub async fn result(&self) -> Result<AgentRunResult> {
        let (a, id) = (self.inner.clone(), self.run_id.clone());
        run(async move { Ok(a.result(&id).await?.into()) }).await
    }

    /// Waits until no turn is running (at most `timeout_ms`), then returns
    /// the result.
    pub async fn wait(&self, timeout_ms: Option<u64>) -> Result<AgentRunResult> {
        let (a, id) = (self.inner.clone(), self.run_id.clone());
        let t = Duration::from_millis(timeout_ms.unwrap_or(30 * 60 * 1000));
        let r: Result<AgentRunResult> = run(async move { Ok(a.wait(&id, t).await?.into()) }).await;
        // Once per handle: the harness id (cua-agents catalog, else
        // `other`), outcome, error category and a duration bucket.
        if !self
            .recorded
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            let (outcome, variant) = match &r {
                Ok(res) if res.error.is_none() => (cua_telemetry::Outcome::Ok, None),
                Ok(_) => (cua_telemetry::Outcome::Error, None),
                Err(e) => (cua_telemetry::Outcome::Error, Some(e.variant())),
            };
            cua_telemetry::capture(cua_telemetry::events::agent_run_completed(
                &self.harness,
                "",
                outcome,
                variant,
                self.started.elapsed(),
            ));
            // Activation: the first agent run that finished cleanly on
            // this install.
            if outcome == cua_telemetry::Outcome::Ok {
                cua_telemetry::global().capture_step("first_agent_run", outcome);
            }
        }
        r
    }

    /// Files the run created or changed in its working directory.
    pub async fn artifacts(&self) -> Result<Vec<AgentArtifact>> {
        let (a, id) = (self.inner.clone(), self.run_id.clone());
        run(async move {
            Ok(a.artifacts(&id)
                .await?
                .into_iter()
                .map(|x| AgentArtifact {
                    path: x.path,
                    size: x.size,
                    modified_ms: x.modified_ms,
                })
                .collect())
        })
        .await
    }

    /// Stops the run and deletes its directory (secrets included).
    pub async fn remove(&self) -> Result<()> {
        let (a, id) = (self.inner.clone(), self.run_id.clone());
        run(async move { Ok(a.remove(&id).await?) }).await
    }
}

#[uniffi::export]
impl SpacesdClient {
    /// Coding agents in this guest.
    pub async fn agents(&self) -> Result<Arc<Agents>> {
        Agents::over(self.client.clone()).await
    }
}
