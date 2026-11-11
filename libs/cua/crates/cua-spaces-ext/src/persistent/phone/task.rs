// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `task_start`, `task_status` and `task_options`: start a task from a
//! phone.
//!
//! The phone picks an agent and where it runs (the default placement,
//! `auto`, one machine, or an existing Space), writes the job and sends
//! it. This is the ask-first flow of the desktop app, run by the daemon so
//! the phone needs no more than this one call:
//!
//! 1. find the Space: the one named, or a new agent home (the slim
//!    headless Linux Space, a tree root with its spawn capability on, made
//!    the way the desktop app makes it) or the agent's home from before;
//! 2. add the agent to it as a member on standby;
//! 3. post the job as `@<handle> <job>` in the Space's thread.
//!
//! Nothing here widens what the account's own app may do:
//!
//! - a new Space is made by the daemon's own create, so the secure
//!   default holds (the internet and the Space's own tree; not the host,
//!   the LAN, the cloud metadata address or other trees), the budgets and
//!   the placement rules apply, and the phone has no argument that widens
//!   any of it;
//! - only the machine's owner creates Spaces and adds agents; an editor of
//!   a Space the account is shared into can post the job to an agent that
//!   is already a member (the same right as typing it in the thread);
//! - what the agent does in the Space is under the Space's own rules:
//!   inside the sandbox it runs on its own, and anything that crosses the
//!   Space's boundary is asked of the person in the thread, as for any
//!   other task.
//!
//! Exactly once: the call carries a key (`client_id`). The same key again
//! returns the first answer and never creates a second Space or posts the
//! job twice. Making a Space can take minutes (an image to pull), so the
//! call answers `creating` at once and `task_status` follows it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::AgentSpec;
use super::super::members::{MemberOptions, PostArgs, may_manage};
use super::super::{Persistent, locked_json};
use cua_sandbox_core::placement::{Kind, On};
use cua_spaces::extension::Caller;
use cua_spaces::mcp::ToolOutcome;
use cua_spaces::{Error, Result, SpaceCreate, SpaceCreated, Spaces};

/// The agent home runs on the slim Linux image (the `cua` CLI and a
/// spawn-capable cua-spacesd are baked into that tier, so the agent can make
/// child Spaces); a test keeps it equal to the image catalog and to the
/// desktop app's `AGENT_HOME_IMAGE` where that exists.
pub const HOME_IMAGE: &str = "ghcr.io/trycua/linux:24.04-slim";
/// The labels the daemon reads on a home (`cua-daemon`'s `children::home`).
pub const LABEL_HOME: &str = "cua.home";
pub const LABEL_AGENT: &str = "cua.home.agent";
/// Records kept, and how long.
const KEEP: usize = 500;
const KEEP_MS: u64 = 7 * 24 * 3600 * 1000;
/// What one account may ask a computer to start from a phone: at once, in an hour, and fresh homes in a day.
/// A stolen or looping phone cannot run the computer (or a linked cloud's bill) up.
const MAX_STARTING: usize = 3;
const MAX_PER_HOUR: usize = 20;
const MAX_FRESH_PER_DAY: usize = 3;
/// A task still `creating` this long after it began was cut off (the
/// daemon stopped meanwhile).
const STALE_MS: u64 = 45 * 60 * 1000;
/// The call waits this long for a quick start (an existing Space) before
/// it answers `creating`.
const QUICK: Duration = Duration::from_secs(4);
/// Longest job text.
pub const PROMPT_MAX: usize = 16 * 1024;

/// An agent a task can start: harness id, the handle it answers to in a
/// Space, its name.
pub struct Startable {
    pub harness: &'static str,
    pub handle: &'static str,
    pub label: &'static str,
}

/// Kept equal to the desktop app's list (`cua-spaces-app-core`'s summon).
pub const AGENTS: &[Startable] = &[
    Startable {
        harness: "claude-code",
        handle: "claude",
        label: "Claude Code",
    },
    Startable {
        harness: "openai-codex",
        handle: "codex",
        label: "Codex",
    },
    Startable {
        harness: "hermes",
        handle: "hermes",
        label: "Hermes",
    },
    Startable {
        harness: "openclaw",
        handle: "openclaw",
        label: "OpenClaw",
    },
];

fn startable(id: &str) -> Option<&'static Startable> {
    let id = match id {
        "codex" => "openai-codex",
        "claude" => "claude-code",
        other => other,
    };
    AGENTS.iter().find(|a| a.harness == id)
}

/// The home's name: a friendly pair (`brave-otter`) no Space has yet. The
/// task is not part of it: a prompt rarely reads well as a name.
pub fn home_name(taken: impl Fn(&str) -> bool) -> String {
    cua_spaces::names::pet_name(taken)
}

/// Where the job runs, as the phone asks.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Where {
    /// The daemon's default placement: this machine, overflowing into a
    /// linked cloud when it is full.
    Default,
    /// The least-loaded machine that can take it.
    Auto,
    Machine(String),
    Space(String),
}

/// `top` is the call's own `space` argument: for `space` it is the Space
/// (the host already checked the caller may reach it and mapped it to its
/// own id; a name inside `where` is never trusted instead).
fn where_of(v: Option<&Value>, top: Option<&str>) -> Result<Where> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok(Where::Default);
    };
    let field = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    match v.get("kind").and_then(Value::as_str).unwrap_or("") {
        "default" | "" => Ok(Where::Default),
        "auto" => Ok(Where::Auto),
        "machine" => {
            Ok(Where::Machine(field("machine").ok_or_else(|| {
                Error::invalid("where.machine: the machine's id")
            })?))
        }
        "space" => Ok(Where::Space(
            top.map(str::to_string)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| Error::invalid("where.space: the Space is the call's `space`"))?,
        )),
        other => Err(Error::invalid(format!(
            "where.kind {other:?}: use \"auto\", \"machine\" or \"space\""
        ))),
    }
}

/// A repository link a job may name: https, no sign-in in it.
fn repo_line(v: Option<&Value>) -> Result<Option<String>> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let url = v.get("url").and_then(Value::as_str).unwrap_or("").trim();
    let parsed = url::Url::parse(url).map_err(|_| Error::invalid("repo.url: an https link"))?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.host_str().is_none()
        || url.len() > 300
    {
        return Err(Error::invalid(
            "repo.url: an https link without a sign-in in it",
        ));
    }
    let mut line = format!("Repo: {url}");
    if let Some(b) = v
        .get("branch")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|b| !b.is_empty())
    {
        if b.len() > 200
            || !b
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._/-".contains(&c))
        {
            return Err(Error::invalid("repo.branch: a branch name"));
        }
        line.push_str(&format!(" (branch {b})"));
    }
    Ok(Some(line))
}

// --------------------------------------------------------------- records

/// What `task_start` did for one `client_id`.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Record {
    /// The account that asked: one account's key never reads another's.
    account: String,
    client_id: String,
    started_ms: u64,
    /// `creating`, `ready` or `failed`.
    state: String,
    #[serde(default)]
    space: Option<String>,
    #[serde(default)]
    entry_seq: Option<u64>,
    #[serde(default)]
    created: bool,
    #[serde(default)]
    error: Option<String>,
    /// The agent's handle, for the phone to open its conversation.
    #[serde(default)]
    handle: Option<String>,
    /// Asked for a home of its own (counts toward the daily cap).
    #[serde(default)]
    fresh: bool,
}

impl Record {
    fn json(&self, created_now: bool) -> Value {
        let mut v = json!({
            "client_id": self.client_id,
            "state": self.state,
            "space": self.space,
            "created": self.created && created_now,
            "entry_seq": self.entry_seq,
            "agent": self.handle,
        });
        if let Some(e) = &self.error {
            v["error"] = json!(e);
        }
        if self.state == "creating" {
            // The key to follow it with.
            v["pending_id"] = json!(self.client_id);
        }
        v
    }
}

fn records_path(p: &Persistent) -> std::path::PathBuf {
    p.dir().join("phone-tasks.json")
}

fn now_ms() -> u64 {
    cua_volume::now_ms()
}

fn load(p: &Persistent, account: &str, client_id: &str) -> Result<Option<Record>> {
    let all: Vec<Record> = super::super::read_json(&records_path(p))?;
    let mut r = all
        .into_iter()
        .find(|r| r.account == account && r.client_id == client_id);
    // A start the daemon never finished (it stopped): say so.
    if let Some(r) = r.as_mut()
        && r.state == "creating"
        && now_ms().saturating_sub(r.started_ms) > STALE_MS
    {
        r.state = "failed".into();
        r.error =
            Some("the task did not finish starting (Cua stopped meanwhile): start it again".into());
    }
    Ok(r)
}

/// Whether the owner turned on spending a linked cloud for tasks started from a phone
/// (`CUA_PHONE_MAY_SPEND=on` in the computer's environment). Off by default.
fn phones_may_spend() -> bool {
    std::env::var("CUA_PHONE_MAY_SPEND").is_ok_and(|v| v == "on")
}

/// Refuses a start past the per-account limits (`task_limit`).
fn check_limits(p: &Persistent, account: &str, fresh: bool) -> Result<()> {
    let all: Vec<Record> = super::super::read_json(&records_path(p))?;
    let now = now_ms();
    let mine = || all.iter().filter(|r| r.account == account);
    let starting = mine()
        .filter(|r| r.state == "creating" && now.saturating_sub(r.started_ms) <= STALE_MS)
        .count();
    if starting >= MAX_STARTING {
        return Err(Error::extension(
            "task_limit",
            format!("{MAX_STARTING} tasks are starting already: wait for one to finish"),
        ));
    }
    if mine()
        .filter(|r| now.saturating_sub(r.started_ms) < 3_600_000)
        .count()
        >= MAX_PER_HOUR
    {
        return Err(Error::extension(
            "task_limit",
            format!("at most {MAX_PER_HOUR} tasks an hour can start from a phone: try again later"),
        ));
    }
    if fresh
        && mine()
            .filter(|r| r.fresh && now.saturating_sub(r.started_ms) < 24 * 3_600_000)
            .count()
            >= MAX_FRESH_PER_DAY
    {
        return Err(Error::extension(
            "task_limit",
            format!("at most {MAX_FRESH_PER_DAY} new homes a day can start from a phone"),
        ));
    }
    Ok(())
}

fn save(p: &Persistent, rec: Record) -> Result<()> {
    locked_json(&records_path(p), move |all: &mut Vec<Record>| {
        let now = now_ms();
        all.retain(|r| now.saturating_sub(r.started_ms) < KEEP_MS);
        match all
            .iter_mut()
            .find(|r| r.account == rec.account && r.client_id == rec.client_id)
        {
            Some(slot) => *slot = rec,
            None => all.push(rec),
        }
        let extra = all.len().saturating_sub(KEEP);
        all.drain(..extra);
    })
}

/// One start at a time per key, so a retry racing the first call waits for it.
fn key_lock(account: &str, client_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> =
        std::sync::OnceLock::new();
    let mut m = LOCKS
        .get_or_init(Default::default)
        .lock()
        .expect("task locks");
    if m.len() > 2048 {
        m.retain(|_, l| Arc::strong_count(l) > 1);
    }
    m.entry(format!("{account}\u{0}{client_id}"))
        .or_default()
        .clone()
}

fn who(caller: &Caller) -> String {
    match caller {
        Caller::Relay { account, .. } => account.clone(),
        _ => "owner".into(),
    }
}

fn client_id_of(a: &Value) -> Result<String> {
    let id = a
        .get("client_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    if id.len() < 8
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:".contains(&b))
    {
        return Err(Error::invalid(
            "client_id: 8 to 128 characters (letters, digits and - _ . :), one per task the person starts",
        ));
    }
    Ok(id.to_string())
}

// ----------------------------------------------------------------- tools

/// `task_start`.
pub(crate) async fn task_start(
    spaces: &Spaces,
    p: &Persistent,
    caller: &Caller,
    a: Value,
) -> Result<ToolOutcome> {
    let client_id = client_id_of(&a)?;
    let account = who(caller);
    let lock = key_lock(&account, &client_id);
    let _one = lock.lock().await;
    if let Some(rec) = load(p, &account, &client_id)? {
        return Ok(ToolOutcome::json(&rec.json(false)));
    }
    let agent = a
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or("claude-code");
    let agent = startable(agent).ok_or_else(|| {
        Error::invalid(format!(
            "agent {agent:?}: one of {}",
            AGENTS
                .iter()
                .map(|a| a.harness)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    let model = a
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(str::to_string);
    if model
        .as_ref()
        .is_some_and(|m| m.len() > 100 || m.chars().any(char::is_control))
    {
        return Err(Error::invalid("model: a model id"));
    }
    let prompt = a
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let message = a
        .get("message")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty());
    if prompt.is_empty() && message.is_none() {
        return Err(Error::invalid("prompt: what the agent should do"));
    }
    if prompt.len() > PROMPT_MAX || message.is_some_and(|m| m.len() > PROMPT_MAX) {
        return Err(Error::invalid(format!(
            "prompt: at most {PROMPT_MAX} bytes"
        )));
    }
    let repo = repo_line(a.get("repo"))?;
    let place = where_of(a.get("where"), a.get("space").and_then(Value::as_str))?;
    let os = a.get("os").and_then(Value::as_str).unwrap_or("linux");
    let fresh = a.get("fresh").and_then(Value::as_bool).unwrap_or(false);
    // Making a Space and adding an agent are the owner's; posting to an
    // agent already in a Space is any editor's.
    let creates = !matches!(place, Where::Space(_));
    if creates {
        may_manage(caller)?;
        if os != "linux" {
            return Err(Error::extension(
                "unsupported_os",
                "a task starts in a Linux Space for now: pick an existing Space for a Mac or Windows one",
            ));
        }
    }
    if let Where::Space(s) = &place {
        // Fails here, before anything is recorded, for a Space the caller
        // may not reach.
        p.thread(caller, s).await?;
    }
    // The limits are checked and the start recorded under one lock, so
    // starts racing each other cannot all pass.
    let limits = key_lock(&account, "limits");
    let _limits = limits.lock().await;
    check_limits(p, &account, fresh && creates)?;
    if creates
        && matches!(place, Where::Auto)
        && !phones_may_spend()
        && spaces.has_linked_clouds().await
    {
        return Err(Error::extension(
            "phone_spend",
            "a task from a phone does not start where a linked cloud could be billed: pick a computer, or turn on spending from phones on the computer",
        ));
    }
    let text = match message {
        Some(m) => m.to_string(),
        None => format!("@{} {}", agent.handle, prompt),
    };
    let text = match &repo {
        Some(r) => format!("{text}\n\n{r}"),
        None => text,
    };
    let rec = Record {
        account: account.clone(),
        client_id: client_id.clone(),
        started_ms: now_ms(),
        state: "creating".into(),
        space: None,
        entry_seq: None,
        created: false,
        error: None,
        handle: Some(agent.handle.to_string()),
        fresh: fresh && creates,
    };
    save(p, rec.clone())?;
    let job = Job {
        spaces: spaces.clone(),
        p: p.clone(),
        caller: caller.clone(),
        record: rec,
        agent,
        model,
        text,
        place,
        fresh,
    };
    let done = tokio::spawn(job.run());
    // Quick when the Space exists already; otherwise the phone follows it.
    match tokio::time::timeout(QUICK, done).await {
        Ok(Ok(rec)) => Ok(ToolOutcome::json(&rec.json(true))),
        _ => {
            let rec =
                load(p, &account, &client_id)?.ok_or_else(|| Error::NotFound("task".into()))?;
            Ok(ToolOutcome::json(&rec.json(true)))
        }
    }
}

/// The start of one task, run to the end whether or not the caller waits.
struct Job {
    spaces: Spaces,
    p: Persistent,
    caller: Caller,
    record: Record,
    agent: &'static Startable,
    model: Option<String>,
    text: String,
    place: Where,
    fresh: bool,
}

impl Job {
    async fn run(mut self) -> Record {
        let outcome = self.start().await;
        match outcome {
            Ok((space, seq, created)) => {
                self.record.state = "ready".into();
                self.record.space = Some(space);
                self.record.entry_seq = Some(seq);
                self.record.created = created;
            }
            Err(e) => {
                self.record.state = "failed".into();
                // The words the person is told: never a stack of causes.
                self.record.error = Some(e.to_string());
            }
        }
        if let Err(e) = save(&self.p, self.record.clone()) {
            tracing::warn!("task record not saved: {e}");
        }
        self.record
    }

    async fn start(&mut self) -> Result<(String, u64, bool)> {
        let (space, created) = match self.place.clone() {
            Where::Space(s) => (self.spaces.resolve(&s)?.to_string(), false),
            other => match self.reusable_home(&other) {
                Some(id) => (id, false),
                None => (self.create_home(&other).await?, true),
            },
        };
        if created {
            self.record.space = Some(space.clone());
            save(&self.p, self.record.clone())?;
        } else {
            self.p.wake_space(&space).await?;
        }
        self.add_agent(&space, matches!(self.place, Where::Space(_)))?;
        let t = self.p.thread(&self.caller, &space).await?;
        let entry = t
            .post(PostArgs {
                text: self.text.clone(),
                ..Default::default()
            })
            .await?;
        Ok((space, entry.seq, created))
    }

    /// The agent's home from an earlier task, still there: tasks of one
    /// agent share a home unless the person asked for a fresh one. Only
    /// for the default placement and `auto`: a machine named is a new
    /// home there.
    fn reusable_home(&self, place: &Where) -> Option<String> {
        if self.fresh || !matches!(place, Where::Default | Where::Auto) {
            return None;
        }
        let rows = self.spaces.list().ok()?;
        rows.into_iter()
            .filter(|s| {
                s.labels.get(LABEL_HOME).is_some_and(|v| v == "1")
                    && s.labels
                        .get(LABEL_AGENT)
                        .is_some_and(|a| a == self.agent.harness)
                    && s.status.is_empty()
                    && s.power_state != "stopped"
            })
            .map(|s| s.id)
            .next()
    }

    async fn create_home(&self, place: &Where) -> Result<String> {
        let here = self.spaces.this_relay_machine_id();
        let (on, kind) = match place {
            // Explicit local: no location given must not overflow into a linked cloud's bill
            // unless the owner allowed phones to spend.
            Where::Default if !phones_may_spend() => (Some(On::Local), Kind::Container),
            Where::Default => (None, Kind::Container),
            Where::Auto => (Some(On::Auto), Kind::Auto),
            Where::Machine(m) if Some(m.as_str()) == here.as_deref() => {
                (Some(On::Local), Kind::Container)
            }
            Where::Machine(m) => (Some(On::Host(m.clone())), Kind::Auto),
            Where::Space(_) => unreachable!("an existing Space is not created"),
        };
        let create = SpaceCreate {
            image: Some(HOME_IMAGE.to_string()),
            on,
            kind,
            name: Some(home_name(|n| {
                self.spaces
                    .list()
                    .map(|v| v.iter().any(|i| i.name == n))
                    .unwrap_or(false)
            })),
            cpus: Some(1),
            memory_mb: Some(1024),
            wait: Some(true),
            spacesd: Some(true),
            headless: true,
            // The key of this task, so a cancel finds the create.
            create_id: Some(self.record.client_id.clone()),
            labels: [
                (LABEL_HOME.to_string(), "1".to_string()),
                (LABEL_AGENT.to_string(), self.agent.harness.to_string()),
                // A home is made for a task: it sleeps when the task is done.
                (
                    cua_spaces::sleep::LABEL_SLEEP.to_string(),
                    cua_spaces::sleep::Sleep::Auto.word(),
                ),
            ]
            .into(),
            // No network, egress or mount argument: the secure default
            // holds, and it is the daemon's own create that applies it.
            ..Default::default()
        };
        let info = match self.spaces.create(create).await? {
            SpaceCreated::Ready { info, .. } => info,
            SpaceCreated::Starting(p) => {
                return Err(Error::Timeout(format!("{} is still starting", p.id)));
            }
        };
        // The spawn capability, when this machine runs the Space: the agent
        // makes its children from here.
        if info.id.starts_with("local:")
            && let Err(e) = self.spaces.set_children(&info.id, true)
        {
            tracing::warn!(space = %info.id, "could not turn on child Spaces: {e}");
        }
        Ok(info.id)
    }

    /// The agent as a member of `space`, on standby (adding it again
    /// updates it). In a Space the caller was given, an agent that is not a
    /// member yet is the owner's to add.
    fn add_agent(&self, space: &str, existing: bool) -> Result<()> {
        let rec = self.p.member_named(space, self.agent.handle)?;
        if rec.is_some() && existing {
            return Ok(());
        }
        self.p.add_member(
            &self.caller,
            AgentSpec {
                name: self.agent.handle.to_string(),
                harness: self.agent.harness.to_string(),
                space: space.to_string(),
                model: self.model.clone(),
                effort: None,
                brief: None,
                base_url: None,
                env_from_host: vec![],
                env: Default::default(),
            },
            MemberOptions::default(),
        )?;
        Ok(())
    }
}

/// `task_status`: how a started task is doing.
pub(crate) async fn task_status(p: &Persistent, caller: &Caller, a: Value) -> Result<ToolOutcome> {
    let client_id = client_id_of(&a)?;
    let rec = load(p, &who(caller), &client_id)?
        .ok_or_else(|| Error::NotFound("no task was started with this client_id".into()))?;
    Ok(ToolOutcome::json(&rec.json(false)))
}

/// `task_options`: what this machine offers a phone for a new task.
pub(crate) async fn task_options(spaces: &Spaces, caller: &Caller) -> Result<ToolOutcome> {
    let owner = may_manage(caller).is_ok();
    // The name the person gave this machine (`cua host setup --name`).
    let name = cua_host::Host::new(spaces.home_dir())
        .config()
        .ok()
        .flatten()
        .map(|c| c.name)
        .filter(|n| !n.is_empty());
    let machine = json!({
        "id": spaces.this_relay_machine_id(),
        "name": name,
        "os": [std::env::consts::OS],
        "online": true,
        "can_create": owner,
        "why": if owner { Value::Null } else { json!("Only the owner can start a task on a new Space") },
    });
    let caps = cua_spaces::agents::capabilities();
    let agents: Vec<Value> = AGENTS
        .iter()
        .map(|a| {
            let info = caps.harnesses.iter().find(|h| h.id == a.harness);
            let ready = info.is_some_and(|h| h.ready);
            json!({
                "harness": a.harness,
                "handle": a.handle,
                "label": a.label,
                "ready": ready,
                // The words come from the connections registry, not the model.
                "why": if ready { Value::Null } else {
                    json!(format!("Sign in to {} in Settings, Connections", a.label))
                },
            })
        })
        .collect();
    Ok(ToolOutcome::json(
        &json!({"machine": machine, "agents": agents}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_home_gets_a_friendly_name_no_space_has() {
        let n = home_name(|_| false);
        assert!(cua_spaces::names::is_pet_name(&n), "{n}");
        // A name that is taken is not handed out again.
        let first = std::cell::RefCell::new(String::new());
        let m = home_name(|c| {
            if first.borrow().is_empty() {
                *first.borrow_mut() = c.to_string();
            }
            c == first.borrow().as_str()
        });
        assert_ne!(m, *first.borrow());
    }

    #[test]
    fn where_is_read_strictly() {
        assert_eq!(where_of(None, None).unwrap(), Where::Default);
        assert_eq!(
            where_of(Some(&json!({"kind": "auto"})), None).unwrap(),
            Where::Auto
        );
        assert_eq!(
            where_of(Some(&json!({"kind": "machine", "machine": "m1"})), None).unwrap(),
            Where::Machine("m1".into())
        );
        assert!(where_of(Some(&json!({"kind": "machine"})), None).is_err());
        assert!(where_of(Some(&json!({"kind": "everywhere"})), None).is_err());
        // The Space is the call's, never one named inside `where`.
        assert_eq!(
            where_of(
                Some(&json!({"kind": "space", "space": "local:other"})),
                Some("local:mine")
            )
            .unwrap(),
            Where::Space("local:mine".into())
        );
        assert!(
            where_of(
                Some(&json!({"kind": "space", "space": "local:other"})),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn a_repo_is_an_https_link_without_a_sign_in() {
        assert_eq!(
            repo_line(Some(
                &json!({"url": "https://github.com/acme/app", "branch": "main"})
            ))
            .unwrap()
            .unwrap(),
            "Repo: https://github.com/acme/app (branch main)"
        );
        for bad in [
            "http://github.com/a/b",
            "https://user:pw@github.com/a/b",
            "git@github.com:a/b.git",
            "file:///x",
        ] {
            assert!(repo_line(Some(&json!({"url": bad}))).is_err(), "{bad}");
        }
        assert!(
            repo_line(Some(
                &json!({"url": "https://github.com/a/b", "branch": "x; rm"})
            ))
            .is_err()
        );
        assert!(repo_line(None).unwrap().is_none());
    }

    #[test]
    fn client_ids_are_checked() {
        assert!(
            client_id_of(&json!({"client_id": "0f8e6c1a-1b2c-4d3e-8f90-123456789abc"})).is_ok()
        );
        for bad in [
            json!({}),
            json!({"client_id": "short"}),
            json!({"client_id": "has space in it ok"}),
            json!({"client_id": "x".repeat(200)}),
        ] {
            assert!(client_id_of(&bad).is_err());
        }
    }

    #[test]
    fn the_agents_are_the_desktop_apps() {
        assert_eq!(startable("codex").unwrap().harness, "openai-codex");
        assert_eq!(startable("claude").unwrap().handle, "claude");
        assert!(startable("nonesuch").is_none());
    }

    /// The home's image and labels are the desktop app's: this fails when
    /// either side changes without the other.
    #[test]
    fn the_home_is_the_desktop_apps_home() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let container = root.join("apps/cua-spaces-web/src/lib/agent-home/container.ts");
        let Ok(container) = std::fs::read_to_string(container) else {
            // A checkout without the web app (a crate tarball) has nothing to compare.
            return;
        };
        assert!(container.contains(&format!("AGENT_HOME_IMAGE = \"{HOME_IMAGE}\"")));
        assert!(
            container.contains("home: \"cua.home\"")
                && container.contains("agent: \"cua.home.agent\"")
        );
        // The commands the app lists for a home.
        for call in [
            "cua spaces ls",
            "cua spaces create <linux|windows|macos> --name <name> [--image <image>]",
            "cua spaces create <os> --on host:<computer>",
        ] {
            assert!(container.contains(call), "{call}");
        }
        // The catalog publishes the image.
        let catalog = std::fs::read_to_string(root.join("libs/images/sandbox-images.json"))
            .unwrap_or_default();
        assert!(catalog.is_empty() || catalog.contains(HOME_IMAGE));
    }
}
