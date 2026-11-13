// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Persistent agents: a named agent whose memory outlives its runs and its
//! Space.
//!
//! A persistent agent is a record (`<cua home>/persistent/agents.json`): a
//! name, a harness, the Space it works in, and how to reach its model. Its
//! home lives in the Cua Volume at `agents/<name>/`. Starting it restores the
//! home into the Space (`~/cua-volume/agents/<name>`), takes the home's
//! single-writer lease, and starts a run whose harness keeps its memory
//! there ([`cua_spaces::agents::harness::memory_dir`]). The supervisor
//! ([`Persistent::tick`], run by `cua daemon`) then, for every live run:
//!
//! - saves the home after each turn, and posts a notification with the
//!   agent's answer ("Your research is ready");
//! - answers the run's bridge (MCP server `cua`: `notify_user`, the drive,
//!   and the user's computers the agent was granted);
//! - fires the agent's routines when they come due, with or without an app
//!   open;
//! - renews the lease.
//!
//! [`Persistent::pause`] stops the run, saves the home, releases the lease,
//! keeps routines from firing, and suspends a local Space. A cloud Space
//! cannot be suspended, so pausing releases it
//! after the home is saved, and [`Persistent::resume`] creates it again
//! from the same image and restores the home.

pub mod access;
pub mod bridge;
pub mod durable;
pub mod home;
pub mod host_tools;
pub mod identity;
pub mod jobs;
pub mod join;
pub mod members;
pub mod notify;
pub mod phases;
pub mod phone;
pub mod push_card;
pub(crate) use phases::DISPATCH_BUDGET;
pub mod rights;
mod sleep;
pub mod wake;

use crate::DriveResult as _;
use crate::SpacesDrive as _;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::thread::{Author, Draft, EntryKind, check_text};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use cua_spaces::agents::{self as ag, Agents, RunOptions, RunStatus};
use cua_spaces::routines::{
    FileStorage, Routine, RoutineFiring, RoutineRunner, RoutineStore, Schedule,
};
use cua_spaces::{Error, Result, SpaceId, Spaces};

pub use members::{InFlight, MemberOptions, Policy, Thread};

/// The directory under the cua home.
pub const DIR: &str = "persistent";
/// How long a home lease lasts without renewal.
pub const LEASE_TTL: Duration = Duration::from_secs(10 * 60);
/// Budget for one agent's share of a supervisor pass.
pub(crate) const AGENT_TICK_BUDGET: Duration = Duration::from_secs(60);
/// How long a start waits on Cua Volume before it starts without it: the
/// home's lease, the agent's folder on the mounted volume (taking the
/// volume, then the folder answering), and a copy of the home. Each waits
/// on the store, and with replication on, on the other computer: while
/// sync has stalled a start used to wait as long as sync did, then fail
/// with "guest command failed (None)" (CUA-4002).
const LEASE_TIMEOUT: Duration = Duration::from_secs(15);
const VOLUME_READY_TIMEOUT: Duration = Duration::from_secs(20);
const VOLUME_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const COPY_FALLBACK_TIMEOUT: Duration = Duration::from_secs(60);

/// Reads a JSON array file (`[]` when missing).
pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    match std::fs::read(path) {
        Ok(b) if b.iter().all(u8::is_ascii_whitespace) => Ok(vec![]),
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}

/// Read-modify-write of a JSON array file under an exclusive lock that
/// every process honors.
pub(crate) fn locked_json<T, R>(path: &Path, f: impl FnOnce(&mut Vec<T>) -> R) -> Result<R>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    cua_home::guard_write(path)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut items: Vec<T> = read_json(path)?;
    let r = f(&mut items);
    cua_home::write_private(path, &serde_json::to_vec_pretty(&items)?)?;
    Ok(r)
}

/// Where the agent's Space is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceState {
    /// Running (or at least not paused by us).
    #[default]
    Running,
    /// A local Space we suspended on pause.
    Suspended,
    /// A cloud Space we released on pause; resume creates it again.
    Released,
    /// The Space was deleted (or is not registered any more): the agent is
    /// paused for good and nothing supervises it. Its home stays in the Cua
    /// Volume; `agent rm` forgets the record.
    Gone,
}

/// What resume needs to create a released cloud Space again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recreate {
    /// The image the Space ran.
    pub image: String,
    /// The Space's name (`cloud:<name>`).
    pub name: String,
}

/// One persistent agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRecord {
    /// The record's key on this machine: unique across Spaces.
    pub name: String,
    /// What its Space calls it (`@claude`), when that is not `name`: a
    /// second Space's `claude` is stored as `claude-2` and still answers to
    /// `@claude` there. Absent: the name is the handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    /// Harness id (`claude-code`, `openai-codex`, `hermes`, `openclaw`, ...).
    pub harness: String,
    /// The Space it works in (a Space id).
    pub space: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Reasoning level the agent's runs take (Claude Code and Codex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// A custom model endpoint base URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Provider key variables forwarded from the host's environment at
    /// each start (names only).
    #[serde(default)]
    pub env_from_host: Vec<String>,
    /// More environment for every run (not secrets).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub created_ms: u64,
    /// Paused: no runs, no routines, no notifications.
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub space_state: SpaceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recreate: Option<Recreate>,
    /// The current run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Event cursor of the current run (the supervisor's bookmark).
    #[serde(default)]
    pub cursor: u64,
    /// The last turn whose home was saved.
    #[serde(default)]
    pub saved_turn: u32,
    /// Post a notification when a turn ends. Default true.
    #[serde(default = "yes")]
    pub notify: bool,
    /// Unix ms of the last home save.
    #[serde(default)]
    pub saved_ms: u64,
    /// The last problem the supervisor hit (cleared on success).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// Session 1's current run started without Cua Volume (it was not
    /// answering): it works in a folder of its own and is never saved over
    /// the home. Cleared by the next start that has the volume.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub home_offline: bool,
    /// The failed turn last notified (`<run>#<turn>`): one notification
    /// per failed turn, however many passes see its events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_error: Option<String>,
    /// As an agent member of its Space: parallel sessions (1..=8). The
    /// run fields above (`run_id`, `cursor`, `saved_turn`, `last_error`,
    /// `notified_error`, `task`, `activity`, `epoch`) are session 1's;
    /// sessions 2 and up keep theirs in `session_runs`.
    #[serde(default = "one")]
    pub sessions: u32,
    /// As an agent member: how it runs.
    #[serde(default)]
    pub policy: Policy,
    /// Standing instructions for its runs (an agent home's: where it is, what
    /// it may do, how to ask). The first message a run takes carries them in
    /// front of the message's text; the thread shows only the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    /// The run the brief was last given to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub briefed_run: Option<String>,
    /// Who added it (absent: the owner, for agents created before members).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_by: Option<crate::thread::Author>,
    /// Unix ms it became a member (set on its first save).
    #[serde(default)]
    pub member_since_ms: u64,
    /// The thread task its current turn works on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<InFlight>,
    /// Who took control of it (an interrupt or stop by name): no task is
    /// delivered until control is given back (`agent_resume`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held_by: Option<crate::thread::Author>,
    /// Who it is ([`identity`]): absent on records from before identities
    /// (read as a started agent; written on the record's next save).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<identity::Stored>,
    /// A durable member's current run: its epoch, its re-drives and its
    /// journal's session on the Volume.
    #[serde(
        default,
        skip_serializing_if = "crate::durable_member::Redrive::is_empty"
    )]
    pub redrive: crate::durable_member::Redrive,
    /// `on: auto`: the durable Space it works in, by its stable name (the
    /// Space's id changes when it moves to another machine).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durable_space: Option<String>,
    /// Its Space's `.cua/setup`: empty (nothing to run), `pending`, `ok` or
    /// `failed` (no tasks are delivered until it is `ok` or empty).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub setup: String,
    /// What the last `.cua/setup` said.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub setup_note: String,
    /// Its durable run was lost with its Space: started again from the
    /// journal on the Volume once the new Space is set up.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub relaunch: bool,
    /// What the current turn is doing (the latest tool or plan step, one
    /// short line, secret-scanned).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// The session's drive epoch (0 or 1: a plain run; a durable re-drive
    /// raises it). Task claims are held at this epoch.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub epoch: u64,
    /// Sessions 2 and up (session 1's run fields are the ones above).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub session_runs: Vec<SessionRun>,
    /// Which session this record shows (not stored): 0 or 1 is session 1;
    /// [`AgentRecord::session`] gives a record whose run fields are
    /// session `k`'s.
    #[serde(skip)]
    pub session_k: u32,
    /// Fields written by a newer daemon, kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// The run fields of session `k` (2 and up) of an agent member.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionRun {
    pub k: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default)]
    pub cursor: u64,
    #[serde(default)]
    pub saved_turn: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<InFlight>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub epoch: u64,
    /// Fields written by a newer daemon, kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// The most sessions an agent member runs.
pub const MAX_SESSIONS: u32 = 8;

impl AgentRecord {
    /// What its Space calls it: the `@handle`.
    pub fn handle(&self) -> &str {
        self.handle.as_deref().unwrap_or(&self.name)
    }

    /// The session this record shows (1-based).
    pub fn k(&self) -> u32 {
        self.session_k.max(1)
    }

    /// The session's drive epoch (at least 1).
    pub fn drive_epoch(&self) -> u64 {
        self.epoch.max(1)
    }

    /// Swaps the run fields with `s`'s.
    fn swap_session(&mut self, s: &mut SessionRun) {
        std::mem::swap(&mut self.run_id, &mut s.run_id);
        std::mem::swap(&mut self.cursor, &mut s.cursor);
        std::mem::swap(&mut self.saved_turn, &mut s.saved_turn);
        std::mem::swap(&mut self.last_error, &mut s.last_error);
        std::mem::swap(&mut self.notified_error, &mut s.notified_error);
        std::mem::swap(&mut self.task, &mut s.task);
        std::mem::swap(&mut self.activity, &mut s.activity);
        std::mem::swap(&mut self.epoch, &mut s.epoch);
    }

    /// This member with session `k`'s run fields (1..=`sessions`).
    pub fn session(&self, k: u32) -> AgentRecord {
        let mut v = self.clone();
        let k = k.max(1);
        if k > 1 {
            let mut s = self
                .session_runs
                .iter()
                .find(|s| s.k == k)
                .cloned()
                .unwrap_or(SessionRun {
                    k,
                    ..Default::default()
                });
            v.swap_session(&mut s);
        }
        v.session_k = k;
        v
    }

    /// Every session of this member, session 1 first.
    pub fn session_views(&self) -> Vec<AgentRecord> {
        (1..=self.sessions.clamp(1, MAX_SESSIONS))
            .map(|k| self.session(k))
            .collect()
    }

    /// Runs `f` on the record with session `k`'s run fields in place.
    fn with_session<R>(&mut self, k: u32, f: impl FnOnce(&mut AgentRecord) -> R) -> R {
        if k <= 1 {
            return f(self);
        }
        let i = match self.session_runs.iter().position(|s| s.k == k) {
            Some(i) => i,
            None => {
                self.session_runs.push(SessionRun {
                    k,
                    ..Default::default()
                });
                self.session_runs.sort_by_key(|s| s.k);
                self.session_runs
                    .iter()
                    .position(|s| s.k == k)
                    .expect("just added")
            }
        };
        let mut s = std::mem::take(&mut self.session_runs[i]);
        self.swap_session(&mut s);
        let r = f(self);
        self.swap_session(&mut s);
        s.k = k;
        self.session_runs[i] = s;
        r
    }
}

fn one() -> u32 {
    1
}

pub(crate) fn yes() -> bool {
    true
}

/// A run `agent_start` started outside a persistent agent, followed by the
/// supervisor until its process exits so that a failed run or turn posts
/// one notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedRun {
    pub space: String,
    pub run_id: String,
    /// Harness id.
    pub agent: String,
    /// Unix ms.
    pub added_ms: u64,
    /// Event cursor (the supervisor's bookmark).
    #[serde(default)]
    pub cursor: u64,
    /// The last failed turn notified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_turn: Option<u32>,
}

/// How long a watched run is followed at most.
pub const WATCH_FOR: Duration = Duration::from_secs(24 * 3600);
/// Watched runs kept (oldest dropped first).
const MAX_WATCHED: usize = 200;

/// How a persistent agent is created.
#[derive(Clone, Debug, Default)]
pub struct AgentSpec {
    pub name: String,
    pub harness: String,
    pub space: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Standing instructions ([`AgentRecord::brief`]).
    pub brief: Option<String>,
    pub base_url: Option<String>,
    pub env_from_host: Vec<String>,
    /// More environment for every run (not secrets).
    pub env: std::collections::BTreeMap<String, String>,
}

/// What [`Persistent::send`] did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Delivery {
    pub run_id: String,
    /// A new run was started (with the home restored) rather than a
    /// follow-up sent.
    pub started: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored: Option<home::Transfer>,
    /// Something the person should know about this start, in plain words
    /// (it started without Cua Volume, and why).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

/// What [`Persistent::pause`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PauseReport {
    pub stopped_run: Option<String>,
    pub saved: Option<home::Transfer>,
    pub space_state: SpaceState,
    pub millis: u64,
}

/// What [`Persistent::resume`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ResumeReport {
    pub space: String,
    /// The cloud Space was created again.
    pub recreated: bool,
    pub restored: Option<home::Transfer>,
    pub run_id: Option<String>,
    /// From the call to the Space being ready with the home restored.
    pub ready_ms: u64,
    /// What `.cua/resume` did (or `.cua/setup`, for a Space created again).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook: Option<crate::hooks::HookReport>,
}

/// One supervisor pass.
#[derive(Clone, Debug, Default, Serialize)]
pub struct TickReport {
    pub fired: Vec<String>,
    pub saved: Vec<String>,
    pub notified: usize,
    pub bridged: usize,
    pub errors: Vec<String>,
}

#[derive(Default)]
pub(crate) struct Live {
    leases: HashMap<String, cua_volume::lease::Lease>,
    agents: HashMap<String, Agents>,
    /// Spaces whose runs did not answer a status read, and when: not asked again for a while.
    unreachable: HashMap<String, std::time::Instant>,
    bridges: HashMap<String, bridge::BridgeState>,
    routines: Option<Arc<tokio::sync::Mutex<RoutineStore>>>,
    routines_mtime: Option<std::time::SystemTime>,
    supervising: Option<std::fs::File>,
    /// Threads already rebuilt from the Volume (or tried) by this process.
    rebuilt: std::collections::HashSet<String>,
    /// Per thread: failed mirror passes in a row, and when to try again.
    mirror_backoff: HashMap<String, (u32, u64)>,
    /// Wrong join codes and connected members' posts.
    pub(crate) join: join::LiveJoin,
    /// Members whose `.cua/setup` runs now.
    setups: std::collections::HashSet<String>,
    /// Journal shippers of durable runs, by `<run>#<epoch>`.
    shippers: HashMap<String, crate::session_journal::Shipper>,
    /// Runs started for a task, until the agent is up: run id to the task's
    /// seq and whether the start is a restart nobody asked for (only that
    /// one says so in the thread; a start the person asked for shows as the
    /// starting row).
    joining: HashMap<String, (u64, bool)>,
    /// Tasks put back after an agent stopped, whose next start is a restart.
    restarted: std::collections::HashSet<u64>,
    /// Agent-to-agent wakes per Space ([`wake`]).
    wake: wake::LiveWake,
    /// Local Spaces some agent names that the registry does not have, and
    /// since when ([`Persistent::reconcile_gone`]).
    missing: HashMap<String, std::time::Instant>,
    /// When a standby run may be started again after one that failed, by member.
    standby_retry: HashMap<String, std::time::Instant>,
    /// The credential hash an agent's run would start with now, by agent
    /// name, and when it was worked out.
    cred_hashes: HashMap<String, (std::time::Instant, Option<String>)>,
    /// Spaces that sleep on their own: what the last checks found ([`sleep`]).
    sleep: HashMap<String, sleep::Watch>,
}

/// How long a worked-out credential hash is reused.
const CRED_HASH_TTL: Duration = Duration::from_secs(5);

/// The persistent agents of one cua home. Cheap to clone.
#[derive(Clone)]
pub struct Persistent {
    pub(crate) spaces: Spaces,
    pub(crate) drive: cua_volume::Drive,
    dir: PathBuf,
    live: Arc<tokio::sync::Mutex<Live>>,
    /// This machine's user as a thread author.
    owner: Arc<std::sync::RwLock<crate::thread::Author>>,
    /// Account ids of the admins of the owner's teams.
    admins: Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    /// What each working session has said so far this turn, for the thread's streaming row
    /// (`live_rows`); gone when the turn ends (the reply is then the real entry).
    partials: Partials,
    /// The supervisor serves each member's queued tasks on a task of its
    /// own instead of inside its pass (see [`Persistent::detached`]).
    detach_dispatch: bool,
    /// Errors of the per-agent passes that ran on their own tasks (detached),
    /// handed to the next pass's report.
    detached_errors: Arc<std::sync::Mutex<Vec<String>>>,
}

/// What each working session has said so far this turn, by `<space>/<name>#<session>`; shared by every
/// handle on one runtime (the tools and the supervisor each hold their own).
pub(crate) type Partials = Arc<std::sync::Mutex<HashMap<String, Partial>>>;

/// A turn's message so far.
#[derive(Clone, Debug, Default)]
pub(crate) struct Partial {
    turn: u32,
    text: String,
    /// The last chunk of `text` seen and when it was first seen at that length: a stretch of prose
    /// nothing has been added to for [`QUIET`] is finished (the agent woke up on its own after a
    /// turn, or stopped mid-thought) and is posted like any other.
    seen_seq: u64,
    seen_at: Option<Instant>,
    /// Subagent tracking (kept under the session's key with a `!subs` suffix): the events that
    /// concern subagents, the fold of them, and whether the run's earlier events were read.
    sub_events: Vec<ag::AgentEvent>,
    subs: Vec<cua_agents::subagents::Subagent>,
    sub_last: u64,
    backfilled: bool,
    /// The run those belong to (a new run numbers its events from 1 again).
    sub_run: String,
}

/// How long a stretch of the agent's words must stay unchanged, with nothing after it, before it
/// is posted as a message of its own.
const QUIET: Duration = Duration::from_millis(1500);

/// The longest streaming text a live row carries (characters): the whole message follows as the reply.
const MAX_PARTIAL: usize = 8_000;

/// How many of the thread's newest entries a pass looks through to see whether it posted a message already.
const RECENT: u64 = 300;

impl std::fmt::Debug for Persistent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Persistent")
            .field("dir", &self.dir)
            .finish()
    }
}

fn host_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "this-computer".into())
}

impl Persistent {
    pub(crate) fn new(
        spaces: Spaces,
        drive: cua_volume::Drive,
        live: Arc<tokio::sync::Mutex<Live>>,
        partials: Partials,
        owner: Arc<std::sync::RwLock<crate::thread::Author>>,
        admins: Arc<std::sync::RwLock<std::collections::HashSet<String>>>,
    ) -> Persistent {
        let dir = spaces.home_dir().join(DIR);
        phases::set_sink(&dir);
        Persistent {
            spaces,
            drive,
            dir,
            live,
            partials,
            owner,
            admins,
            detach_dispatch: false,
            detached_errors: Arc::default(),
        }
    }

    /// This handle with queued tasks served on tasks of their own: a pass
    /// of [`Persistent::tick`] starts each member's delivery and goes on
    /// (the daemon's supervisor), where the default pass waits for them.
    pub fn detached(mut self) -> Persistent {
        self.detach_dispatch = true;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn agents_path(&self) -> PathBuf {
        self.dir.join("agents.json")
    }

    /// The notifications feed.
    pub fn feed(&self) -> notify::Feed {
        notify::Feed::new(&self.dir)
    }

    /// Per-agent access to the user's computers.
    pub fn access(&self) -> access::Access {
        access::Access::new(&self.dir)
    }

    fn routines_path(&self) -> PathBuf {
        self.dir.join("routines.json")
    }

    /// Every persistent agent.
    pub fn list(&self) -> Result<Vec<AgentRecord>> {
        read_json(&self.agents_path())
    }

    /// One persistent agent.
    pub fn get(&self, name: &str) -> Result<AgentRecord> {
        self.list()?
            .into_iter()
            .find(|a| a.name == name)
            .ok_or_else(|| Error::NotFound(format!("persistent agent {name}")))
    }

    /// [`Self::update`] of the session `rec` shows (its run fields).
    fn update_in<R>(&self, rec: &AgentRecord, f: impl FnOnce(&mut AgentRecord) -> R) -> Result<R> {
        let k = rec.k();
        self.update(&rec.name, |r| r.with_session(k, f))
    }

    fn update<R>(&self, name: &str, f: impl FnOnce(&mut AgentRecord) -> R) -> Result<R> {
        let owner = self.owner();
        locked_json(&self.agents_path(), |all: &mut Vec<AgentRecord>| {
            all.iter_mut().find(|a| a.name == name).map(|a| {
                // An agent from before members becomes one on its first save.
                if a.member_since_ms == 0 {
                    a.member_since_ms = cua_volume::now_ms();
                }
                let r = f(a);
                // ...and gets the identity it was read as.
                if a.identity.is_none() {
                    a.identity = Some(identity::stored_of(a, &owner));
                }
                r
            })
        })?
        .ok_or_else(|| Error::NotFound(format!("persistent agent {name}")))
    }

    /// Creates a persistent agent (its home is created on first save).
    pub fn create(&self, spec: AgentSpec) -> Result<AgentRecord> {
        if !cua_volume::path::valid_agent_name(&spec.name) {
            return Err(Error::invalid(format!(
                "agent name {:?}: use 1-63 of a-z, 0-9, `.`, `_`, `-`",
                spec.name
            )));
        }
        let h = ag::harness::harness(&spec.harness).ok_or_else(|| {
            Error::invalid(format!(
                "unknown harness {:?}; known: {}",
                spec.harness,
                ag::harness::ids().join(", ")
            ))
        })?;
        let space = self.spaces.resolve(&spec.space)?.to_string();
        let allowed = cua_spaces::agents::forwardable_env();
        let saved = if spec.env_from_host.is_empty() {
            vec![]
        } else {
            cua_spaces::agents::keys::global().names()
        };
        for k in &spec.env_from_host {
            cua_spaces::agents::refuse_host_only(k)?;
            if !allowed.contains(&k.as_str()) && !saved.contains(k) {
                return Err(Error::invalid(format!(
                    "{k} is not a provider key variable (or a key saved in Cua Spaces → Settings → Connections)"
                )));
            }
        }
        let rec = AgentRecord {
            name: spec.name.clone(),
            handle: None,
            harness: h.id.into(),
            space,
            model: spec.model,
            effort: spec.effort,
            base_url: spec.base_url,
            env_from_host: spec.env_from_host,
            env: spec.env,
            brief: spec.brief,
            briefed_run: None,
            created_ms: cua_volume::now_ms(),
            paused: false,
            space_state: SpaceState::Running,
            recreate: None,
            run_id: None,
            cursor: 0,
            saved_turn: 0,
            notify: true,
            saved_ms: 0,
            last_error: None,
            home_offline: false,
            notified_error: None,
            sessions: 1,
            policy: Policy::default(),
            added_by: None,
            member_since_ms: cua_volume::now_ms(),
            task: None,
            held_by: None,
            identity: None,
            redrive: Default::default(),
            durable_space: None,
            setup: String::new(),
            setup_note: String::new(),
            relaunch: false,
            activity: None,
            epoch: 0,
            session_runs: vec![],
            session_k: 0,
            extra: Default::default(),
        };
        let out = rec.clone();
        locked_json(&self.agents_path(), |all: &mut Vec<AgentRecord>| {
            if all.iter().any(|a| a.name == rec.name) {
                return Err(Error::invalid(format!(
                    "a persistent agent named {} exists",
                    rec.name
                )));
            }
            all.push(rec);
            Ok(())
        })??;
        Ok(out)
    }

    /// Forgets a persistent agent (its home stays in the drive). Its run
    /// is stopped and its routines removed.
    pub async fn remove(&self, name: &str) -> Result<AgentRecord> {
        let rec = self.get(name)?;
        if identity::is_external(&rec) {
            // A connected agent has no run, home or routines here: dropping
            // its record stops its member key at once.
            if let Err(e) = self.fail_open_tasks(&rec, &format!("{name} was removed")) {
                tracing::warn!(agent = name, "open tasks not closed: {e}");
            }
            locked_json(&self.agents_path(), |all: &mut Vec<AgentRecord>| {
                all.retain(|a| a.name != name)
            })?;
            return Ok(rec);
        }
        if !rec.paused {
            for s in rec.session_views() {
                if s.run_id.is_some() {
                    let _ = self.stop_run(&s).await;
                }
            }
        }
        if let Err(e) = self.fail_open_tasks(&rec, &format!("{name} was removed")) {
            tracing::warn!(agent = name, "open tasks not closed: {e}");
        }
        self.release_lease(name).await;
        // A home on the mounted volume: its writes land, and the volume
        // shows the Space's own view again.
        let _ = self.spaces.volume_release_agent(&rec.space, name).await;
        // Its home stays, but without the drive's bookkeeping (the reserved
        // sync manifest, the lease), so the user can delete it entirely.
        let _ = self.drive.forget_agent_home(name).await;
        let store = self.routine_store().await?;
        {
            let mut s = store.lock().await;
            for r in s.routines_for(name) {
                s.delete(&r.id);
            }
        }
        locked_json(&self.agents_path(), |all: &mut Vec<AgentRecord>| {
            all.retain(|a| a.name != name)
        })?;
        Ok(rec)
    }

    /// A Space was deleted: the persistent agents that lived in it go with
    /// it, as `agent rm` would (run stopped, open tasks failed, routines and
    /// standby sessions dropped, record forgotten; the agent's home stays
    /// in the Cua Volume). An agent whose Space is only being replaced is
    /// kept: a cloud Space released on pause (`recreate`) and a durable
    /// member, whose copy the coordinator renews. Returns the names removed.
    pub async fn space_deleted(&self, space: &str) -> Vec<String> {
        let Ok(all) = self.list() else {
            return vec![];
        };
        let mut removed = vec![];
        for rec in all.into_iter().filter(|r| {
            r.space == space
                && r.recreate.is_none()
                && r.space_state != SpaceState::Released
                && !r.policy.durable
                && r.durable_space.is_none()
        }) {
            match tokio::time::timeout(Duration::from_secs(30), self.remove(&rec.name)).await {
                Ok(Ok(_)) => {
                    tracing::info!(agent = %rec.name, space, "persistent agents: removed an agent whose Space was deleted");
                    removed.push(rec.name);
                }
                Ok(Err(e)) => {
                    tracing::warn!(agent = %rec.name, space, "persistent agents: could not remove an agent of a deleted Space: {e}")
                }
                Err(_) => {
                    tracing::warn!(agent = %rec.name, space, "persistent agents: removing an agent of a deleted Space timed out")
                }
            }
        }
        removed
    }

    /// Marks `gone` the agents whose local Space is no longer registered
    /// (deleted behind the supervisor's back: the sandbox CLI, an older
    /// daemon, a crash between the delete and its cleanup), once the Space
    /// has been missing for `grace` (a Space deleted and created again under
    /// its name is not missed). A gone agent is paused and unsupervised, its
    /// runs, sessions and routines are dropped, its home stays in the Cua
    /// Volume, and `agent rm` forgets it. Returns the names marked.
    pub async fn reconcile_gone(&self, grace: Duration) -> Vec<String> {
        let (Ok(all), Ok(known)) = (self.list(), self.spaces.list()) else {
            return vec![];
        };
        let known: std::collections::HashSet<String> = known.into_iter().map(|s| s.id).collect();
        let now = Instant::now();
        let mut due = vec![];
        {
            let mut live = self.live.lock().await;
            live.missing.retain(|space, _| {
                all.iter()
                    .any(|r| &r.space == space && r.space_state != SpaceState::Gone)
                    && !known.contains(space)
            });
            for rec in all.iter().filter(|r| {
                r.space_state != SpaceState::Gone
                    && r.space.starts_with("local:")
                    && !known.contains(&r.space)
                    && r.recreate.is_none()
                    && !identity::is_external(r)
            }) {
                let since = *live.missing.entry(rec.space.clone()).or_insert(now);
                if now.duration_since(since) >= grace {
                    due.push(rec.clone());
                }
            }
        }
        let mut marked = vec![];
        for rec in due {
            let name = rec.name.clone();
            if let Err(e) = self.fail_open_tasks(&rec, &format!("{name}'s Space was deleted")) {
                tracing::warn!(agent = %name, "open tasks not closed: {e}");
            }
            self.release_lease(&name).await;
            if let Ok(store) = self.routine_store().await {
                let mut s = store.lock().await;
                for r in s.routines_for(&name) {
                    s.delete(&r.id);
                }
            }
            let why = format!(
                "its Space {} was deleted; its home stays in the Cua Volume (`agent rm {name}` forgets the agent)",
                rec.space
            );
            let done = self.update(&name, |r| {
                r.space_state = SpaceState::Gone;
                r.paused = true;
                r.run_id = None;
                r.activity = None;
                r.session_runs.clear();
                r.relaunch = false;
                r.last_error = Some(why.clone());
            });
            match done {
                Ok(()) => {
                    tracing::info!(agent = %name, space = %rec.space, "persistent agents: {why}");
                    marked.push(name);
                }
                Err(e) => tracing::warn!(agent = %name, "could not mark an agent gone: {e}"),
            }
        }
        marked
    }

    fn session(&self, rec: &AgentRecord) -> cua_volume::Session {
        self.drive.session(crate::drive_runtime::agent_context(
            &self.spaces,
            &rec.name,
            Some(&rec.space),
        ))
    }

    /// A fresh agent runner for `space` (one guest round trip), remembered
    /// for the supervisor. User-initiated calls always take a fresh one, so
    /// a Space deleted and created again is never reached through a stale
    /// connection.
    async fn agents_for(&self, space: &str) -> Result<Agents> {
        let s = self.spaces.space(space).await?;
        let a = s.agents().await?;
        self.live
            .lock()
            .await
            .agents
            .insert(space.to_string(), a.clone());
        Ok(a)
    }

    /// The supervisor's runner for `space`: remembered between passes and
    /// dropped when a pass fails.
    async fn cached_agents(&self, space: &str) -> Result<Agents> {
        if let Some(a) = self.live.lock().await.agents.get(space) {
            return Ok(a.clone());
        }
        self.agents_for(space).await
    }

    /// What the Space runs on, as a telemetry word.
    pub(crate) fn runtime_word(&self, space: &str) -> String {
        self.spaces
            .list()
            .ok()
            .and_then(|l| l.into_iter().find(|s| s.id == space))
            .map(|s| phases::runtime_word(&s.os, &s.kind))
            .unwrap_or("other")
            .to_string()
    }

    async fn forget_space(&self, space: &str) {
        self.live.lock().await.agents.remove(space);
        let _ = self.spaces.forget_connection(space).await;
    }

    async fn take_lease(&self, rec: &AgentRecord) -> Result<()> {
        let holder = format!("{}/{}", host_name(), rec.space);
        let lease = cua_volume::lease::Lease::acquire(
            &self.drive,
            &rec.name,
            &holder,
            LEASE_TTL.as_millis() as u64,
        )
        .await
        .drive()?;
        self.live
            .lock()
            .await
            .leases
            .insert(rec.name.clone(), lease);
        Ok(())
    }

    /// Releases the home's lease when `rec` shows session 1 (the home's
    /// one writer; the other sessions never take it).
    async fn release_writer(&self, rec: &AgentRecord) {
        if rec.k() == 1 {
            self.release_lease(&rec.name).await;
        }
    }

    /// Gives the home's lease back, within [`LEASE_TIMEOUT`]: one the
    /// store could not take back (sync stalled) runs out at its TTL.
    async fn release_lease(&self, name: &str) {
        let lease = self.live.lock().await.leases.remove(name);
        if let Some(l) = lease {
            let _ = tokio::time::timeout(LEASE_TIMEOUT, l.release(&self.drive)).await;
        }
    }

    /// Renews the home's lease when half its TTL is gone. The store is
    /// asked off the `live` lock (every start and message takes it) and
    /// within [`LEASE_TIMEOUT`].
    async fn renew_lease(&self, name: &str) -> Result<()> {
        let due = {
            let live = self.live.lock().await;
            live.leases
                .get(name)
                .filter(|l| {
                    l.info.expires_ms < cua_volume::now_ms() + LEASE_TTL.as_millis() as u64 / 2
                })
                .cloned()
        };
        let Some(mut lease) = due else {
            return Ok(());
        };
        match tokio::time::timeout(
            LEASE_TIMEOUT,
            lease.renew(&self.drive, LEASE_TTL.as_millis() as u64),
        )
        .await
        {
            Ok(r) => r.drive()?,
            Err(_) => {
                return Err(Error::Agent(format!(
                    "Cua Volume didn't answer within {} s while keeping {name}'s home",
                    LEASE_TIMEOUT.as_secs()
                )));
            }
        }
        if let Some(held) = self.live.lock().await.leases.get_mut(name) {
            *held = lease;
        }
        Ok(())
    }

    /// Session 1's home on its Space's mounted Cua Volume for a start:
    /// `Ok(Some(dir))` when the volume is the agent's and its folder
    /// answers, `Ok(None)` when the Space has no volume for it (the home is
    /// copied in and out), `Err(why)` when Cua Volume is not ready (not
    /// taken or not answering in time; the volume is given back).
    async fn ready_mounted_home(
        &self,
        rec: &AgentRecord,
        agents: &Agents,
    ) -> std::result::Result<Option<String>, String> {
        let taken = tokio::time::timeout(
            VOLUME_READY_TIMEOUT,
            self.spaces.volume_for_agent(&rec.space, &rec.name),
        )
        .await;
        let v = match taken {
            Ok(Ok(Some(v))) => v,
            Ok(Ok(None)) => return Ok(None),
            Ok(Err(e)) => {
                self.let_go_of_volume(rec);
                return Err(e.to_string());
            }
            Err(_) => {
                self.let_go_of_volume(rec);
                return Err(format!(
                    "Cua Volume didn't answer within {} s",
                    VOLUME_READY_TIMEOUT.as_secs()
                ));
            }
        };
        let dir = format!("{}/agents/{}", v.mount_path.trim_end_matches('/'), rec.name);
        match home::probe_mounted(agents.guest(), &dir, VOLUME_PROBE_TIMEOUT).await {
            Ok(()) => Ok(Some(dir)),
            Err(e) => {
                self.let_go_of_volume(rec);
                Err(format!("its folder on Cua Volume isn't answering: {e}"))
            }
        }
    }

    /// Gives `rec`'s Space its own view of the volume back, in the
    /// background (it waits on the store).
    fn let_go_of_volume(&self, rec: &AgentRecord) {
        let (spaces, space, name) = (self.spaces.clone(), rec.space.clone(), rec.name.clone());
        tokio::spawn(async move {
            if let Err(e) = spaces.volume_release_agent(&space, &name).await {
                tracing::warn!(agent = %name, "Cua Volume not given back to the Space yet: {e}");
            }
        });
    }

    /// The agent's home on its Space's mounted Cua Volume, when the Space
    /// has one this agent can hold (`<mount>/agents/<name>`); `None` means
    /// the home is copied in and out.
    async fn mounted_home(&self, rec: &AgentRecord) -> Option<String> {
        let taken = tokio::time::timeout(
            VOLUME_READY_TIMEOUT,
            self.spaces.volume_for_agent(&rec.space, &rec.name),
        )
        .await
        .unwrap_or_else(|_| {
            Err(Error::Agent(format!(
                "Cua Volume didn't answer within {} s",
                VOLUME_READY_TIMEOUT.as_secs()
            )))
        });
        match taken {
            Ok(Some(v)) => {
                return Some(format!(
                    "{}/agents/{}",
                    v.mount_path.trim_end_matches('/'),
                    rec.name
                ));
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(agent = %rec.name, "copying the home: the volume is unavailable: {e}")
            }
        }
        None
    }

    /// Restores the home into the agent's Space (without starting a run).
    /// Nothing moves when the home is on the Space's mounted volume.
    pub async fn restore(&self, name: &str) -> Result<home::Transfer> {
        let rec = self.get(name)?;
        self.not_external(&rec)?;
        let t0 = Instant::now();
        if self.mounted_home(&rec).await.is_some() {
            return Ok(home::Transfer::mounted(t0.elapsed().as_millis() as u64));
        }
        let agents = self.agents_for(&rec.space).await?;
        let dir = agents.agent_home_dir(&rec.name);
        home::restore(agents.guest(), &self.session(&rec), &rec.name, &dir).await
    }

    /// Saves the agent's home from its Space into the drive now (on the
    /// mounted volume: lands the writes still pending).
    pub async fn save(&self, name: &str) -> Result<home::Transfer> {
        let rec = self.get(name)?;
        self.not_external(&rec)?;
        let t0 = Instant::now();
        // A run that started without the volume never has the home's
        // files: saving it would remove them from the home.
        if rec.home_offline {
            return Ok(home::Transfer::offline(t0.elapsed().as_millis() as u64));
        }
        let flushed = self
            .spaces
            .volume_flush_agent(&rec.space, &rec.name)
            .await?;
        let t = if flushed {
            home::Transfer::mounted(t0.elapsed().as_millis() as u64)
        } else {
            let agents = self.agents_for(&rec.space).await?;
            let dir = agents.agent_home_dir(&rec.name);
            home::save(agents.guest(), &self.session(&rec), &rec.name, &dir).await?
        };
        self.update(name, |r| {
            r.saved_ms = cua_volume::now_ms();
            r.last_error = None;
        })?;
        Ok(t)
    }

    /// Starts a run of `name` on `prompt`: takes the home's lease, restores
    /// the home, and starts the harness with its memory in the home and
    /// the bridge attached.
    pub async fn start(&self, name: &str, prompt: &str) -> Result<Delivery> {
        self.start_from(name, prompt, "persistent").await
    }

    /// [`Self::start`], recording `entry` (`persistent` or `routine`) as
    /// where the run was started from.
    async fn start_from(&self, name: &str, prompt: &str, entry: &str) -> Result<Delivery> {
        self.start_with(name, prompt, entry, None).await
    }

    /// [`Self::start_from`]; `restore`: a durable member's journal from the
    /// Volume and the epoch to continue it at.
    async fn start_with(
        &self,
        name: &str,
        prompt: &str,
        entry: &str,
        restore: Option<(u64, std::collections::BTreeMap<String, Vec<u8>>)>,
    ) -> Result<Delivery> {
        let rec = self.get(name)?;
        self.not_external(&rec)?;
        if restore.is_some() && !rec.policy.durable {
            return Err(Error::invalid(format!("{name} is not durable")));
        }
        Box::pin(self.start_session(&rec, prompt, entry, restore)).await
    }

    /// Starts a run of the session `view` shows. Session 1 is the home's
    /// one writer: it takes the home's lease and works in the home (on the
    /// mounted volume when there is one), and only its turns are saved.
    /// Sessions 2 and up start from a copy of the home as last saved, in
    /// their own folder next to it, and never write the home. `restore`: a
    /// durable member's journal from the Volume and the epoch to continue
    /// it at (session 1 only; a durable run is session 1's).
    async fn start_session(
        &self,
        view: &AgentRecord,
        prompt: &str,
        entry: &str,
        restore: Option<(u64, std::collections::BTreeMap<String, Vec<u8>>)>,
    ) -> Result<Delivery> {
        let k = view.k();
        let rec = self.get(&view.name)?.session(k);
        let name = rec.name.as_str();
        if rec.paused {
            return Err(Error::Agent(format!(
                "{name} is paused; resume it first (agent_resume)"
            )));
        }
        // A Space the host put to sleep wakes for the message.
        self.wake_space(&rec.space).await?;
        // Why this run starts without Cua Volume, when it does: the volume
        // did not answer in time (sync with the other computer stalled).
        let mut offline: Option<String> = None;
        if k == 1 {
            match tokio::time::timeout(LEASE_TIMEOUT, self.take_lease(&rec)).await {
                Ok(Ok(())) => {}
                // Another machine works in the home: that is not a stall.
                Ok(Err(e)) if e.tag() == "lease_held" => return Err(e),
                Ok(Err(e)) => offline = Some(e.to_string()),
                Err(_) => {
                    offline = Some(format!(
                        "Cua Volume didn't answer within {} s",
                        LEASE_TIMEOUT.as_secs()
                    ))
                }
            }
        }
        let agents = match self.agents_for(&rec.space).await {
            Ok(a) => a,
            Err(e) => {
                self.release_writer(&rec).await;
                return Err(e);
            }
        };
        let t0 = Instant::now();
        let woke = phases::take_woke(&self.dir, &format!("{}#{k}", view.name));
        let posted_ms = woke
            .as_ref()
            .map(|w| w.posted_ms)
            .unwrap_or_else(phases::now_ms);
        let picked_ms = woke.as_ref().map(|w| w.picked_ms).unwrap_or(posted_ms);
        // A prewarm of this agent's tools that is running now finishes first.
        phases::wait_for_prewarm(
            &self.dir,
            &rec.space,
            &rec.harness,
            Duration::from_secs(300),
        )
        .await;
        // Session 1 works on the mounted volume when its folder there
        // answers; one that does not is a volume that is not ready.
        let mut not_ready: Option<String> = None;
        let mounted = if k == 1 && offline.is_none() {
            match self.ready_mounted_home(&rec, &agents).await {
                Ok(m) => m,
                Err(why) => {
                    not_ready = Some(why);
                    None
                }
            }
        } else {
            None
        };
        let mut home_dir = match &mounted {
            Some(m) => Some(m.clone()),
            None if k > 1 => Some(session_home_dir(&agents, name, k)),
            None => None,
        };
        let restored = if mounted.is_some() {
            Ok(home::Transfer::mounted(t0.elapsed().as_millis() as u64))
        } else if offline.is_some() {
            Ok(home::Transfer::offline(t0.elapsed().as_millis() as u64))
        } else {
            let dir = home_dir
                .clone()
                .unwrap_or_else(|| agents.agent_home_dir(&rec.name));
            let session = self.session(&rec);
            // The home comes back while the files every start shares
            // (bridge, runner, skills) go in: they do not depend on it.
            let copy = futures_util::future::join(
                home::restore(agents.guest(), &session, &rec.name, &dir),
                async {
                    if let Some(h) = cua_agents::harness::harness(&rec.harness) {
                        let _ = agents.prewarm(h, true).await;
                    }
                },
            );
            // A copy right after the volume did not answer, and a later
            // session's (it takes no lease, which would have asked the
            // store first), may wait on a stalled store: bounded, and
            // without it the run starts without the volume.
            if not_ready.is_some() || k > 1 {
                match tokio::time::timeout(COPY_FALLBACK_TIMEOUT, copy).await {
                    Ok((Ok(t), _)) => Ok(t),
                    Ok((Err(e), _)) => {
                        offline = Some(not_ready.take().unwrap_or_else(|| e.to_string()));
                        Ok(home::Transfer::offline(t0.elapsed().as_millis() as u64))
                    }
                    Err(_) => {
                        offline = Some(not_ready.take().unwrap_or_else(|| {
                            format!(
                                "Cua Volume didn't answer within {} s",
                                COPY_FALLBACK_TIMEOUT.as_secs()
                            )
                        }));
                        Ok(home::Transfer::offline(t0.elapsed().as_millis() as u64))
                    }
                }
            } else {
                copy.await.0
            }
        };
        let prepared_ms = phases::now_ms();
        let restored = match restored {
            Ok(t) => t,
            Err(e) => {
                self.release_writer(&rec).await;
                return Err(e);
            }
        };
        if let Some(why) = &offline {
            // A folder of its own on this computer: the home's copy here
            // (if any) is not touched, and nothing of this run is saved
            // over the home.
            home_dir = Some(offline_home_dir(&agents, name, k));
            tracing::warn!(agent = name, "starting without Cua Volume: {why}");
            // An offline run never writes the home: its lease goes back.
            self.release_writer(&rec).await;
        }
        // Its env, its env_from_host keys, and the keys saved in Cua Spaces
        // that its harness reads.
        let (env, missing) = cua_spaces::agents::run_env(
            &rec.harness,
            &rec.env,
            &rec.env_from_host,
            rec.base_url.is_some(),
        )?;
        if !missing.is_empty() {
            self.release_writer(&rec).await;
            return Err(Error::invalid(format!(
                "not set in this server's environment: {}",
                missing.join(", ")
            )));
        }
        let endpoint = rec.base_url.clone().map(|base_url| ag::Endpoint {
            base_url,
            wire: None,
            model: rec.model.clone(),
        });
        let (durable, redrive) = if rec.policy.durable && k == 1 {
            let (d, r) = Self::durable_run(&rec, restore);
            (Some(d), r)
        } else {
            (None, Default::default())
        };
        let started = agents
            .start(
                &rec.harness,
                prompt,
                RunOptions {
                    env,
                    endpoint,
                    model: rec.model.clone(),
                    effort: rec.effort.clone(),
                    home: Some(rec.name.clone()),
                    home_dir,
                    bridge: true,
                    label: Some(if k == 1 {
                        rec.name.clone()
                    } else {
                        format!("{} (session {k})", rec.name)
                    }),
                    durable,
                    // A standby run waits for its messages for as long as it lives.
                    idle_exit_secs: self.idle_exit_for(&rec),
                    ..Default::default()
                },
            )
            .await;
        let started = match started {
            Ok(s) => {
                cua_spaces::agents::telemetry::started(&agents, &rec.space, entry, &s);
                let spawned_ms = phases::now_ms();
                let since = |t: u64| t.saturating_sub(posted_ms);
                phases::follow(
                    agents.clone(),
                    phases::Join {
                        run_id: s.run_id.clone(),
                        harness: rec.harness.clone(),
                        runtime: self.runtime_word(&rec.space),
                        cold: true,
                        posted_ms,
                    },
                    vec![
                        ("posted", 0),
                        ("dispatch_picked", since(picked_ms)),
                        ("host_prepared", since(prepared_ms)),
                        ("guest_spawned", since(spawned_ms)),
                    ],
                    since(spawned_ms),
                );
                s
            }
            Err(e) => {
                self.release_writer(&rec).await;
                let e = Error::from(e);
                cua_spaces::agents::telemetry::start_failed(&rec.harness, &rec.space, entry, &e);
                let msg = e.to_string();
                let _ = self.update_in(&rec, |r| r.last_error = Some(msg.clone()));
                return Err(e);
            }
        };
        self.update_in(&rec, |r| {
            r.run_id = Some(started.run_id.clone());
            r.cursor = 0;
            r.saved_turn = 0;
            r.last_error = None;
            r.redrive = redrive;
            r.activity = None;
        })?;
        if k == 1 {
            let offline = offline.is_some();
            self.update(&rec.name, |r| r.home_offline = offline)?;
        }
        let notice = offline.as_deref().map(|why| offline_notice(name, why));
        if let Some(text) = &notice {
            let _ = self
                .thread_store(&rec.space)
                .append(crate::thread::Draft::note("dispatch", None, text.clone()));
        }
        Ok(Delivery {
            run_id: started.run_id,
            started: true,
            restored: Some(restored),
            notice,
        })
    }

    /// Gives `name` a turn: a follow-up to its idle run, or a new run (with
    /// the home restored) when it has none. Refused while a turn runs or
    /// while paused.
    pub async fn send(&self, name: &str, text: &str) -> Result<Delivery> {
        self.send_from(name, text, "persistent").await
    }

    /// [`Self::send`], recording `entry` for a run it starts.
    async fn send_from(&self, name: &str, text: &str, entry: &str) -> Result<Delivery> {
        let rec = self.get(name)?;
        self.not_external(&rec)?;
        self.send_session(&rec, text, entry, None).await
    }

    /// [`Self::send_from`] to the session `view` shows. `known` is the run's
    /// status as the caller read it a moment ago (the dispatcher just did):
    /// with it a warm delivery reads nothing more from the guest and is one
    /// write.
    async fn send_session(
        &self,
        view: &AgentRecord,
        text: &str,
        entry: &str,
        known: Option<ag::RunInfo>,
    ) -> Result<Delivery> {
        let rec = self.get(&view.name)?.session(view.k());
        let name = rec.name.as_str();
        if rec.paused {
            return Err(Error::Agent(format!("{name} is paused")));
        }
        self.wake_space(&rec.space).await?;
        if let Some(run) = &rec.run_id {
            let agents = self.agents_for(&rec.space).await?;
            let status = match known {
                Some(st) => Ok(st),
                None => agents.status(run).await,
            };
            match status {
                Ok(st) if st.status == RunStatus::Running => {
                    return Err(Error::Agent(format!(
                        "{name} is in the middle of a turn; try again when it ends"
                    )));
                }
                Ok(st) if Self::redriving(&rec, st.status) => {
                    return Err(Error::Agent(format!(
                        "{name} is in the middle of a turn (it crashed and is being resumed); try again when it ends"
                    )));
                }
                // An idle run whose credentials are not the ones forwarded
                // now (the person reconnected in Settings), or whose last
                // turn failed to authenticate, ends and starts again: it
                // would otherwise keep using what it began with.
                Ok(st)
                    if st.accepts_message
                        && st.status == RunStatus::Idle
                        && self.credentials_changed(&rec, &agents, run, &st).await =>
                {
                    tracing::info!(
                        agent = name,
                        "credentials changed or rejected: starting a fresh run"
                    );
                    if rec.k() == 1 {
                        let _ = self.save(name).await;
                    }
                    if let Err(e) = agents.stop(run).await {
                        tracing::warn!(agent = name, "the old run did not stop: {e}");
                    }
                    self.release_writer(&rec).await;
                }
                // An idle run whose home is not where this agent's home is
                // now (it was on the mounted Volume, which shows the agent's
                // view only while the agent holds it): sending would start
                // its runner again in a folder that is not there. The home
                // is taken first (the Volume shows this agent's view), and a
                // run on another home ends and starts again.
                Ok(st)
                    if st.accepts_message
                        && st.status == RunStatus::Idle
                        && rec.k() == 1
                        && self.home_moved(&rec, &st).await =>
                {
                    tracing::info!(agent = name, "the run's home moved: starting a fresh run");
                    let _ = self.save(name).await;
                    if let Err(e) = agents.stop(run).await {
                        tracing::warn!(agent = name, "the old run did not stop: {e}");
                    }
                    self.release_writer(&rec).await;
                }
                Ok(st) if st.accepts_message && st.status == RunStatus::Idle => {
                    if rec.k() == 1 && !self.live.lock().await.leases.contains_key(name) {
                        self.take_lease(&rec).await?;
                    }
                    agents
                        .send_after(run, text, vec![], &st, rec.model.as_deref())
                        .await?;
                    return Ok(Delivery {
                        run_id: run.clone(),
                        started: false,
                        restored: None,
                        notice: None,
                    });
                }
                // Failed, crashed, gone or unreadable: start fresh (the
                // home carries the memory).
                _ => {}
            }
        }
        Box::pin(self.start_session(&rec, text, entry, None)).await
    }

    /// Whether the idle run `st` of `rec` works on the mounted Volume in a
    /// home other than the one `rec` has now (or the Volume is not its to
    /// hold). Takes the Volume for the agent when it has one, so a
    /// run on the mounted home finds its folders where it left them.
    async fn home_moved(&self, rec: &AgentRecord, st: &ag::RunInfo) -> bool {
        let Some(cwd) = st.meta.as_ref().map(|m| m.cwd.as_str()) else {
            return false;
        };
        // Only a run on the mounted Volume can lose its folders this way.
        let on_volume = cwd.starts_with("/volume/");
        match self.mounted_home(rec).await {
            Some(m) => on_volume && !Self::in_home(cwd, &m),
            None => on_volume,
        }
    }

    /// Whether `cwd` is `home` or inside it.
    fn in_home(cwd: &str, home: &str) -> bool {
        let home = home.trim_end_matches('/');
        cwd == home || cwd.strip_prefix(home).is_some_and(|r| r.starts_with('/'))
    }

    /// Whether the idle run `run` of `rec` must not take another message:
    /// it was started with other credentials than the ones forwarded now, or
    /// its last turn failed to authenticate.
    async fn credentials_changed(
        &self,
        rec: &AgentRecord,
        agents: &cua_spaces::agents::Agents,
        run: &str,
        st: &ag::RunInfo,
    ) -> bool {
        if let Some(started) = st.meta.as_ref().and_then(|m| m.cred_hash.as_deref()) {
            // Reading the keys costs a keychain round trip, and a message
            // does not need them fresher than a few seconds.
            let cached = self
                .live
                .lock()
                .await
                .cred_hashes
                .get(&rec.name)
                .filter(|(at, _)| at.elapsed() < CRED_HASH_TTL)
                .map(|(_, h)| h.clone());
            let now = match cached {
                Some(h) => h,
                None => {
                    let h = cua_spaces::agents::run_env(
                        &rec.harness,
                        &rec.env,
                        &rec.env_from_host,
                        rec.base_url.is_some(),
                    )
                    .ok()
                    .map(|(env, _)| ag::runs::credential_hash(&rec.harness, &env));
                    self.live
                        .lock()
                        .await
                        .cred_hashes
                        .insert(rec.name.clone(), (Instant::now(), h.clone()));
                    h
                }
            };
            if now.is_some_and(|h| h != started) {
                return true;
            }
        }
        if st.stop_reason.as_deref() == Some("error")
            && let Ok(r) = agents.result(run).await
        {
            return r.error.as_deref().is_some_and(ag::failure::is_auth_failure);
        }
        false
    }

    async fn stop_run(&self, rec: &AgentRecord) -> Result<Option<String>> {
        let Some(run) = &rec.run_id else {
            return Ok(None);
        };
        let agents = self.agents_for(&rec.space).await?;
        cua_spaces::agents::telemetry::before_stop(&agents, run).await;
        agents.stop(run).await?;
        cua_spaces::agents::telemetry::stopped(run);
        Ok(Some(run.clone()))
    }

    /// Pauses `name` in one call: stops its run, saves its home, releases
    /// the lease, keeps its routines from firing, and suspends a local
    /// Space (a cloud Space is released; resume creates it again).
    pub async fn pause(&self, name: &str) -> Result<PauseReport> {
        let t0 = Instant::now();
        let rec = self.get(name)?;
        self.not_external(&rec)?;
        if rec.space_state == SpaceState::Gone {
            return Err(Error::invalid(format!(
                "{name}'s Space was deleted; remove the agent with `agent rm {name}`"
            )));
        }
        if rec.paused {
            return Ok(PauseReport {
                space_state: rec.space_state,
                ..Default::default()
            });
        }
        // Routines and notifications stop first, so nothing starts the
        // agent again while it is being paused.
        self.update(name, |r| r.paused = true)?;
        // Its tasks go back to the queue and wait for the resume.
        for s in rec.session_views() {
            if let Err(e) = self.requeue(&s, &format!("{name} was paused")) {
                tracing::warn!(agent = name, "task not requeued: {e}");
            }
        }
        let mut report = PauseReport::default();
        let result: Result<()> = async {
            for s in rec.session_views().iter().skip(1) {
                if s.run_id.is_some() {
                    self.stop_run(s).await?;
                }
            }
            report.stopped_run = self.stop_run(&rec).await?;
            report.saved = Some(self.save(name).await?);
            self.release_lease(name).await;
            self.spaces
                .volume_release_agent(&rec.space, name)
                .await?;
            let id = self.spaces.resolve(&rec.space)?;
            match &id {
                SpaceId::Local { name: sandbox } => {
                    // The guest goes to sleep: unmount first.
                    self.spaces.volume_detach(&rec.space).await?;
                    self.forget_space(&rec.space).await;
                    self.spaces
                        .sandboxes()
                        .suspend(sandbox)
                        .await
                        .map_err(Error::Sandbox)?;
                    report.space_state = SpaceState::Suspended;
                }
                SpaceId::Cloud { name: claim, .. } => {
                    let image = self
                        .spaces
                        .list()?
                        .into_iter()
                        .find(|s| s.id == rec.space)
                        .map(|s| s.image)
                        .filter(|i| !i.is_empty())
                        .ok_or_else(|| {
                            Error::invalid(format!(
                                "{} has no recorded image, so it could not be created again; not releasing it",
                                rec.space
                            ))
                        })?;
                    self.forget_space(&rec.space).await;
                    // Recorded before the delete: a Space's deletion removes
                    // the agents that lived in it, except one waiting to be
                    // created again ([`Persistent::space_deleted`]).
                    let recreate = Recreate {
                        image,
                        name: claim.clone(),
                    };
                    self.update(name, |r| r.recreate = Some(recreate))?;
                    if let Err(e) = self.spaces.delete(&rec.space).await {
                        let _ = self.update(name, |r| r.recreate = None);
                        return Err(e);
                    }
                    report.space_state = SpaceState::Released;
                }
                // Someone's own machine or an added address: only the agent
                // pauses.
                _ => report.space_state = SpaceState::Running,
            }
            Ok(())
        }
        .await;
        let state = report.space_state;
        self.update(name, |r| {
            r.space_state = state;
            r.run_id = None;
            r.activity = None;
            for s in &mut r.session_runs {
                s.run_id = None;
                s.activity = None;
            }
            r.last_error = result.as_ref().err().map(|e| e.to_string());
        })?;
        result?;
        report.millis = t0.elapsed().as_millis() as u64;
        Ok(report)
    }

    /// Resumes `name`: resumes (or creates again) its Space, restores its
    /// home, lets its routines fire again, and when `prompt` is given
    /// starts a run on it.
    pub async fn resume(&self, name: &str, prompt: Option<&str>) -> Result<ResumeReport> {
        let t0 = Instant::now();
        let rec = self.get(name)?;
        self.not_external(&rec)?;
        let mut report = ResumeReport {
            space: rec.space.clone(),
            ..Default::default()
        };
        match rec.space_state {
            SpaceState::Gone => {
                return Err(Error::invalid(format!(
                    "{name}'s Space was deleted; its home stays in the Cua Volume. Remove the agent with `agent rm {name}`"
                )));
            }
            SpaceState::Running => {}
            SpaceState::Suspended => {
                if let SpaceId::Local { name: sandbox } = self.spaces.resolve(&rec.space)? {
                    self.spaces
                        .sandboxes()
                        .resume(&sandbox)
                        .await
                        .map_err(Error::Sandbox)?;
                    self.forget_space(&rec.space).await;
                }
            }
            SpaceState::Released => {
                let re = rec.recreate.clone().ok_or_else(|| {
                    Error::invalid(format!(
                        "{name}'s Space was released without a record of its image"
                    ))
                })?;
                let created = self
                    .spaces
                    .create(cua_spaces::SpaceCreate {
                        image: Some(re.image.clone()),
                        on: Some(cua_sandbox_core::placement::On::Cloud),
                        name: Some(re.name.clone()),
                        wait: Some(true),
                        ..Default::default()
                    })
                    .await?;
                let info = match created {
                    cua_spaces::SpaceCreated::Ready { info, .. } => info,
                    cua_spaces::SpaceCreated::Starting(p) => {
                        return Err(Error::Timeout(format!("{} is still starting", p.id)));
                    }
                };
                report.recreated = true;
                report.space = info.id.clone();
                self.update(name, |r| {
                    r.space = info.id.clone();
                    r.recreate = None;
                })?;
            }
        }
        self.update(name, |r| {
            r.space_state = SpaceState::Running;
            r.paused = false;
        })?;
        report.restored = Some(self.restore(name).await?);
        let rec = self.get(name)?;
        if report.recreated {
            // Created again from scratch: .cua/setup prepares it again
            // (a failed one holds the member's tasks).
            report.hook = self.run_setup(&rec).await?;
        } else {
            // The Space kept its state: .cua/resume restarts what a
            // suspend stopped (a dev server, a database).
            match self.run_hook(&rec, crate::hooks::HookKind::Resume).await {
                Ok(h) => report.hook = h,
                Err(e) => tracing::warn!(agent = name, ".cua/resume: {e}"),
            }
        }
        report.ready_ms = t0.elapsed().as_millis() as u64;
        if let Some(p) = prompt {
            report.run_id = Some(self.start(name, p).await?.run_id);
        }
        Ok(report)
    }

    // --- routines -------------------------------------------------------

    async fn routine_store(&self) -> Result<Arc<tokio::sync::Mutex<RoutineStore>>> {
        let mut live = self.live.lock().await;
        let path = self.routines_path();
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if let Some(store) = live.routines.clone() {
            if live.routines_mtime != mtime {
                store.lock().await.load();
                live.routines_mtime = mtime;
            }
            return Ok(store);
        }
        std::fs::create_dir_all(&self.dir)?;
        let mut store = RoutineStore::new(Box::new(FileStorage(path)));
        store.load();
        store.attach(Arc::new(Runner(self.clone())));
        let store = Arc::new(tokio::sync::Mutex::new(store));
        live.routines = Some(store.clone());
        live.routines_mtime = mtime;
        Ok(store)
    }

    async fn after_routine_write(&self) {
        let mut live = self.live.lock().await;
        live.routines_mtime = std::fs::metadata(self.routines_path())
            .and_then(|m| m.modified())
            .ok();
    }

    /// Adds a routine for `agent`.
    pub async fn routine_add(
        &self,
        agent: &str,
        title: &str,
        prompt: &str,
        schedule: Schedule,
        enabled: bool,
    ) -> Result<Routine> {
        self.not_external(&self.get(agent)?)?;
        if schedule.next_fire(Utc::now()).is_none() {
            return Err(Error::invalid("that schedule never fires"));
        }
        let store = self.routine_store().await?;
        let r = store
            .lock()
            .await
            .create(agent, title, prompt, schedule, enabled, Utc::now());
        self.after_routine_write().await;
        Ok(r)
    }

    /// Routines (of one agent, or all).
    pub async fn routines(&self, agent: Option<&str>) -> Result<Vec<Routine>> {
        let store = self.routine_store().await?;
        let s = store.lock().await;
        Ok(match agent {
            Some(a) => s.routines_for(a),
            None => {
                let mut all: Vec<Routine> = self
                    .list()?
                    .iter()
                    .flat_map(|a| s.routines_for(&a.name))
                    .collect();
                all.sort_by_key(|r| r.created_at);
                all
            }
        })
    }

    /// Removes a routine.
    pub async fn routine_remove(&self, id: &str) -> Result<()> {
        let store = self.routine_store().await?;
        let mut s = store.lock().await;
        if s.routine(id).is_none() {
            return Err(Error::NotFound(format!("routine {id}")));
        }
        s.delete(id);
        drop(s);
        self.after_routine_write().await;
        Ok(())
    }

    /// Turns a routine on or off.
    pub async fn routine_set_enabled(&self, id: &str, enabled: bool) -> Result<Routine> {
        let store = self.routine_store().await?;
        let mut s = store.lock().await;
        if s.routine(id).is_none() {
            return Err(Error::NotFound(format!("routine {id}")));
        }
        s.set_enabled(id, enabled);
        let r = s.routine(id).cloned().expect("present");
        drop(s);
        self.after_routine_write().await;
        Ok(r)
    }

    // --- runs started with agent_start --------------------------------

    fn watched_path(&self) -> PathBuf {
        self.dir.join("watched-runs.json")
    }

    /// Follows `run_id` (an `agent_start` run in `space` of harness
    /// `agent`) until its process exits; the supervisor posts one
    /// notification for each failed turn, or for the run failing.
    pub fn watch_run(&self, space: &str, run_id: &str, agent: &str) -> Result<()> {
        let w = WatchedRun {
            space: space.into(),
            run_id: run_id.into(),
            agent: agent.into(),
            added_ms: cua_volume::now_ms(),
            cursor: 0,
            notified_turn: None,
        };
        locked_json(&self.watched_path(), |all: &mut Vec<WatchedRun>| {
            if !all
                .iter()
                .any(|x| x.run_id == w.run_id && x.space == w.space)
            {
                all.push(w);
            }
            let extra = all.len().saturating_sub(MAX_WATCHED);
            all.drain(..extra);
        })
    }

    /// The runs being followed.
    pub fn watched_runs(&self) -> Result<Vec<WatchedRun>> {
        read_json(&self.watched_path())
    }

    /// One pass over the watched runs.
    async fn tick_watched(&self, report: &mut TickReport) {
        let runs = match self.watched_runs() {
            Ok(r) => r,
            Err(e) => {
                report.errors.push(format!("watched runs: {e}"));
                return;
            }
        };
        if runs.is_empty() {
            return;
        }
        let mut next: HashMap<(String, String), Option<WatchedRun>> = HashMap::new();
        for w in runs {
            let key = (w.space.clone(), w.run_id.clone());
            let expired =
                cua_volume::now_ms().saturating_sub(w.added_ms) > WATCH_FOR.as_millis() as u64;
            if expired {
                next.insert(key, None);
                continue;
            }
            match tokio::time::timeout(AGENT_TICK_BUDGET, self.tick_watched_run(w, report)).await {
                Ok(Ok(w)) => {
                    next.insert(key, w);
                }
                Ok(Err(e)) => {
                    self.forget_space(&key.0).await;
                    report.errors.push(format!("{}: {e}", key.1));
                }
                Err(_) => report
                    .errors
                    .push(format!("{}: supervisor pass timed out", key.1)),
            }
        }
        let saved = locked_json(&self.watched_path(), |all: &mut Vec<WatchedRun>| {
            all.retain_mut(|x| match next.get(&(x.space.clone(), x.run_id.clone())) {
                Some(Some(w)) => {
                    *x = w.clone();
                    true
                }
                Some(None) => false,
                None => true,
            });
        });
        if let Err(e) = saved {
            report.errors.push(format!("watched runs: {e}"));
        }
    }

    /// Reads `w`'s new events and status; posts what failed. `None` once
    /// there is nothing left to follow.
    async fn tick_watched_run(
        &self,
        mut w: WatchedRun,
        report: &mut TickReport,
    ) -> Result<Option<WatchedRun>> {
        let agents = self.cached_agents(&w.space).await?;
        let name = ag::harness::harness(&w.agent).map_or(w.agent.as_str(), |h| h.name);
        let page = agents.events(&w.run_id, w.cursor, 500).await?;
        for (turn, err) in failed_turns(&page.events) {
            if w.notified_turn.is_none_or(|t| turn > t) {
                self.post_stopped(&w, name, &err)?;
                w.notified_turn = Some(turn);
                report.notified += 1;
            }
        }
        w.cursor = page.cursor;
        if !page.caught_up {
            return Ok(Some(w));
        }
        let info = match agents.status(&w.run_id).await {
            Ok(info) => info,
            // Removed: nothing to follow.
            Err(cua_agents::Error::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        // A run that failed without an error event (the runner recorded
        // why in its state).
        if info.status == RunStatus::Failed && w.notified_turn.is_none_or(|t| info.turn > t) {
            let err = info.error.clone().unwrap_or_else(|| info.reason.clone());
            self.post_stopped(&w, name, &err)?;
            w.notified_turn = Some(info.turn);
            report.notified += 1;
        }
        Ok((info.alive != Some(false)).then_some(w))
    }

    fn post_stopped(&self, w: &WatchedRun, name: &str, err: &str) -> Result<()> {
        self.feed().post(
            None,
            "error",
            &notify::stopped_title(name, err),
            err,
            Some(&w.run_id),
            Some(&w.space),
        )?;
        Ok(())
    }

    // --- the supervisor -------------------------------------------------

    /// Takes the supervisor lock for this cua home (one supervising
    /// process: the daemon). `false` when another process holds it.
    pub async fn claim_supervisor(&self) -> Result<bool> {
        let mut live = self.live.lock().await;
        if live.supervising.is_some() {
            return Ok(true);
        }
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join("supervisor.lock");
        cua_home::guard_write(&path)?;
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        match f.try_lock() {
            Ok(()) => {
                live.supervising = Some(f);
                Ok(true)
            }
            Err(std::fs::TryLockError::WouldBlock) => Ok(false),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }

    /// One supervisor pass: fire due routines, then for every live run
    /// answer its bridge, save the home after each finished turn (and
    /// notify), and renew its lease. Only the process holding the
    /// supervisor lock does anything.
    pub async fn tick(&self) -> TickReport {
        let mut report = TickReport::default();
        match self.claim_supervisor().await {
            Ok(true) => {}
            Ok(false) => return report,
            Err(e) => {
                report.errors.push(format!("supervisor lock: {e}"));
                return report;
            }
        }
        match self.routine_store().await {
            Ok(store) => {
                for f in cua_spaces::routines::tick_shared(&store, Utc::now()).await {
                    report
                        .fired
                        .push(format!("{}: {}", f.title, f.firing.summary()));
                }
                self.after_routine_write().await;
            }
            Err(e) => report.errors.push(format!("routines: {e}")),
        }
        self.reconcile_gone(GONE_GRACE).await;
        let records = match self.list() {
            Ok(r) => r,
            Err(e) => {
                report.errors.push(format!("agents: {e}"));
                return report;
            }
        };
        // A Space someone stopped or suspended is left alone: its durable
        // members wait for it to run again instead of waking it.
        let off = self.spaces_off();
        let asleep = self.asleep_spaces();
        if self.detach_dispatch {
            // A posted message is handed to its member first, ahead of
            // anything below that can wait on a Space; each session is read
            // on a task of its own (see `serve_sessions`).
            self.dispatch(&mut report).await;
            self.serve_sessions(&records, &off, &asleep, &mut report);
        } else {
            let live_sessions: Vec<AgentRecord> = records
                .iter()
                .filter(|r| !r.paused && !sleep::waits_for_space(r, &off, &asleep))
                .flat_map(|r| r.session_views())
                .filter(|r| r.run_id.is_some())
                .collect();
            for rec in live_sessions {
                self.tick_one(rec, &mut report).await;
            }
        }
        // Durable members whose run is lost with their Space.
        if let Ok(all) = self.list() {
            for rec in all
                .into_iter()
                .filter(|r| r.relaunch && !r.paused && !off.contains(&r.space))
            {
                let name = rec.name.clone();
                match tokio::time::timeout(AGENT_TICK_BUDGET, Box::pin(self.tick_relaunch(&rec)))
                    .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => report.errors.push(format!("{name} relaunch: {e}")),
                    Err(_) => report.errors.push(format!("{name} relaunch: timed out")),
                }
            }
        }
        self.tick_setups(&mut report).await;
        self.tick_standby(&mut report, false).await;
        self.tick_sleep(&mut report).await;
        // Thread tasks for members that are free (after the turns that
        // ended above closed theirs).
        if !self.detach_dispatch {
            self.dispatch(&mut report).await;
        }
        self.tick_watched(&mut report).await;
        self.tick_watched_jobs(&mut report).await;
        self.tick_mirror(&mut report).await;
        report
    }

    /// Starts a task for every live session whose last one has finished:
    /// each is read on its own, so a Space that does not answer (a minute,
    /// at worst) holds up only its own agent. While an agent talks to its
    /// bridge (its tool list at start, then every tool call) its session is
    /// read every quarter second instead of every pass, so a call is
    /// answered at once; ten quiet seconds go back to the pass.
    fn serve_sessions(
        &self,
        records: &[AgentRecord],
        off: &std::collections::BTreeSet<String>,
        asleep: &std::collections::BTreeSet<String>,
        report: &mut TickReport,
    ) {
        report.errors.append(
            &mut self
                .detached_errors
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        );
        let live_sessions = records
            .iter()
            .filter(|r| !r.paused && !sleep::waits_for_space(r, off, asleep))
            .flat_map(|r| r.session_views())
            .filter(|r| r.run_id.is_some());
        for rec in live_sessions {
            let key = format!("{}#{}\u{0}tick", rec.name, rec.k());
            let Some(guard) = phases::ServingGuard::take(&self.dir, &key) else {
                continue; // the earlier pass for it is still running
            };
            let me = self.clone();
            tokio::spawn(async move {
                let _guard = guard;
                // While the agent is talking to its bridge (its tool list at
                // start, then every tool call) the session is read every
                // quarter second instead of every pass, so a call is
                // answered at once; ten quiet seconds go back to the pass.
                let (name, k) = (rec.name.clone(), rec.k());
                let mut rec = rec;
                let mut fast_until: Option<Instant> = None;
                loop {
                    let mut r = TickReport::default();
                    me.tick_one(rec.clone(), &mut r).await;
                    if r.bridged > 0 {
                        fast_until = Some(Instant::now() + Duration::from_secs(10));
                    }
                    if !r.errors.is_empty() {
                        me.detached_errors
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .append(&mut r.errors);
                        break;
                    }
                    if !fast_until.is_some_and(|t| Instant::now() < t) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    match me.get(&name) {
                        Ok(fresh) if !fresh.paused && fresh.session(k).run_id.is_some() => {
                            rec = fresh.session(k);
                        }
                        _ => break,
                    }
                }
            });
        }
    }

    /// One session's pass, within its budget.
    async fn tick_one(&self, rec: AgentRecord, report: &mut TickReport) {
        let name = match rec.k() {
            1 => rec.name.clone(),
            k => format!("{} session {k}", rec.name),
        };
        let view = rec.clone();
        match tokio::time::timeout(AGENT_TICK_BUDGET, self.tick_agent(rec, report)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let msg = e.to_string();
                let _ = self.update_in(&view, |r| r.last_error = Some(msg.clone()));
                report.errors.push(format!("{name}: {msg}"));
            }
            Err(_) => report
                .errors
                .push(format!("{name}: supervisor pass timed out")),
        }
    }

    /// Hands the queued tasks to their members now, outside a pass (only the
    /// process holding the supervisor lock does).
    pub async fn dispatch_now(&self) {
        if !matches!(self.claim_supervisor().await, Ok(true)) {
            return;
        }
        let mut report = TickReport::default();
        // A new member is settled (no `.cua/setup` hook) and its standby run
        // started here, ahead of the pass, which can be held up by a Space
        // that does not answer; a member with a setup hook is left to the pass.
        let _ = tokio::time::timeout(Duration::from_secs(10), self.tick_setups(&mut report)).await;
        self.tick_standby(&mut report, true).await;
        self.dispatch(&mut report).await;
        if let (Ok(records), off) = (self.list(), self.spaces_off()) {
            let asleep = self.asleep_spaces();
            self.serve_sessions(&records, &off, &asleep, &mut report);
        }
        if !report.errors.is_empty() {
            self.detached_errors
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .append(&mut report.errors);
        }
    }

    /// The Spaces of this machine that are stopped or suspended now.
    fn spaces_off(&self) -> std::collections::BTreeSet<String> {
        self.spaces
            .list()
            .map(|l| {
                l.into_iter()
                    .filter(|s| space_is_off(&s.power_state))
                    .map(|s| s.id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Starts the run of every member kept on standby that has none (or
    /// whose run is gone): the agent process, its handshake and its session,
    /// no prompt. A message later finds a session ready. A start that fails
    /// is tried again after half a minute.
    async fn tick_standby(&self, _report: &mut TickReport, fresh_only: bool) {
        let Ok(records) = self.list() else { return };
        let off = self.spaces_off();
        let wanted: Vec<AgentRecord> = records
            .into_iter()
            .filter(|r| {
                r.policy.standby
                    && !r.paused
                    && r.held_by.is_none()
                    && !r.relaunch
                    && r.sessions <= 1
                    && !off.contains(&r.space)
                    && r.setup != durable::setup::PENDING
                    && r.setup != durable::setup::FAILED
                    && !identity::is_external(r)
                    && (!fresh_only || r.run_id.is_none())
            })
            .collect();
        for rec in wanted {
            // Only when nothing is queued for it (a task starts the run with
            // its own prompt) and no start is under way.
            let busy_until = self.live.lock().await.standby_retry.get(&rec.name).copied();
            if busy_until.is_some_and(|t| Instant::now() < t) {
                continue;
            }
            let up = match rec.run_id.as_deref() {
                None => false,
                Some(run) => match self.cached_agents(&rec.space).await {
                    Ok(agents) => agents
                        .status(run)
                        .await
                        .map(|st| st.alive != Some(false) || st.status == RunStatus::Running)
                        .unwrap_or(true),
                    Err(_) => true,
                },
            };
            if up {
                continue;
            }
            let has_task = self
                .thread_store(&rec.space)
                .fold()
                .map(|f| !f.open_tasks(Some(&rec.name)).is_empty())
                .unwrap_or(false);
            if has_task {
                continue;
            }
            let Some(guard) = phases::ServingGuard::take(&self.dir, &rec.name) else {
                continue;
            };
            self.live
                .lock()
                .await
                .standby_retry
                .insert(rec.name.clone(), Instant::now() + Duration::from_secs(30));
            let me = self.clone();
            tokio::spawn(async move {
                let _guard = guard;
                let t0 = Instant::now();
                tracing::info!(agent = %rec.name, "standby run starting");
                match Box::pin(me.start_session(&rec, "", "standby", None)).await {
                    Ok(d) => {
                        me.live.lock().await.standby_retry.remove(&rec.name);
                        tracing::info!(agent = %rec.name, run = %d.run_id, ms = t0.elapsed().as_millis() as u64, "standby run started");
                    }
                    Err(e) => {
                        tracing::warn!(agent = %rec.name, ms = t0.elapsed().as_millis() as u64, "standby run did not start: {e}")
                    }
                }
            });
        }
    }

    async fn tick_agent(&self, rec: AgentRecord, report: &mut TickReport) -> Result<()> {
        let space = rec.space.clone();
        let follow = rec.policy.durable && rec.durable_space.is_some() && !rec.relaunch;
        let r = self.tick_agent_inner(rec.clone(), report).await;
        if r.is_err() {
            // Reconnect on the next pass (the Space may have been recreated).
            self.forget_space(&space).await;
            // A durable Space that moved: the member follows it.
            if follow && let Err(e) = Box::pin(self.tick_relaunch(&rec)).await {
                report.errors.push(format!("{} relaunch: {e}", rec.name));
            }
        }
        r
    }

    /// Posts every message the agent has finished saying, each as an entry of its own, in order,
    /// under the task its turn works on (nothing is replaced or left out), and keeps the one it is
    /// still saying as the thread's live entry. A message is finished when a tool call, a thought or
    /// the end of the turn follows it, or when it stayed unchanged for [`QUIET`]. Returns whether
    /// one is still open (the caller then keeps its event bookmark, so a restart reads it whole).
    fn post_said(&self, rec: &AgentRecord, run: &str, events: &[ag::AgentEvent]) -> Result<bool> {
        let key = format!("{}/{}#{}", rec.space, rec.name, rec.k());
        let said = cua_agents::subagents::said(events);
        let mut open = false;
        let mut changed = false;
        let mut post: Vec<cua_agents::subagents::Said> = vec![];
        {
            let mut map = self.partials.lock().unwrap_or_else(|p| p.into_inner());
            let p = map.entry(key.clone()).or_default();
            for (i, s) in said.iter().enumerate() {
                let last = i + 1 == said.len();
                if !s.closed && last {
                    if p.seen_seq != s.last_seq {
                        p.seen_seq = s.last_seq;
                        p.seen_at = Some(Instant::now());
                    }
                    if p.seen_at.is_some_and(|t| t.elapsed() >= QUIET) {
                        post.push(s.clone());
                    } else {
                        open = true;
                        let shown: String = s.text.chars().take(MAX_PARTIAL).collect();
                        if p.text != shown || p.turn != s.turn {
                            p.turn = s.turn;
                            p.text = shown;
                            changed = true;
                        }
                    }
                } else {
                    post.push(s.clone());
                }
            }
            if !open && !p.text.is_empty() {
                p.text.clear();
                changed = true;
            }
            if !open {
                p.seen_seq = 0;
                p.seen_at = None;
            }
        }
        if !post.is_empty() {
            self.append_said(rec, run, &post)?;
            changed = true;
        }
        if changed {
            self.thread_store(&rec.space).wake();
        }
        Ok(open)
    }

    /// Writes `said` to the thread as the agent's messages (see [`Self::post_said`]). Idempotent:
    /// each carries a key naming the run, the first chunk and how much of it was posted.
    fn append_said(
        &self,
        rec: &AgentRecord,
        run: &str,
        said: &[cua_agents::subagents::Said],
    ) -> Result<()> {
        let store = self.thread_store(&rec.space);
        let fold = store.fold()?;
        let reply_to = rec.task.as_ref().map(|t| t.seq).or_else(|| {
            fold.tasks
                .values()
                .filter(|t| t.to == rec.name)
                .map(|t| t.seq)
                .max()
        });
        let me = Author::agent(&rec.name);
        let last = store.last_seq()?;
        let recent = store.read(last.saturating_sub(RECENT), RECENT as usize + 1)?;
        let mut drafts = vec![];
        // The first stretch of a run is its opening line; every later one is either the closing message
        // or narration between tool calls ("Let me look at it..."), which the thread does not carry.
        let opening = said.iter().map(|s| s.first_seq).min();
        for s in said {
            if Some(s.first_seq) != opening && is_process_narration(&s.text) {
                continue;
            }
            let prefix = format!("seg:{run}:{}:", s.first_seq);
            // How much of this stretch an earlier pass posted (it grows only when the agent paused
            // mid-sentence for longer than [`QUIET`]).
            let done = recent
                .iter()
                .filter_map(|e| {
                    e.client_id
                        .as_deref()?
                        .strip_prefix(prefix.as_str())?
                        .parse::<usize>()
                        .ok()
                })
                .max()
                .unwrap_or(0);
            let total = s.text.chars().count();
            if total <= done {
                continue;
            }
            let text: String = s.text.chars().skip(done).collect();
            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }
            // Already in the thread word for word: the agent posted it itself with `thread_reply` in this
            // turn. That is its latest message after the task it is answering; an identical message
            // from an earlier task (or with other words said since) is a different message.
            let floor = reply_to.unwrap_or(0);
            if recent
                .iter()
                .rev()
                .find(|e| e.seq > floor && e.author.id == me.id && e.kind == EntryKind::Message)
                .is_some_and(|e| e.text.trim() == text)
            {
                continue;
            }
            let text = if check_text(&text).is_ok() {
                text
            } else {
                "(A message of mine looked like it held a secret, so it was not posted.)".into()
            };
            let mut d = Draft::new(me.clone(), EntryKind::Message, text);
            d.reply_to = reply_to;
            d.client_id = Some(format!("{prefix}{total}"));
            drafts.push(d);
        }
        store.append_many(drafts)?;
        Ok(())
    }

    /// Records in the thread each child Space the member's Space made while the member works on a
    /// task (one note each, with a link the clients draw), so a Space made inside a Space is visible
    /// where the person is looking. Children come from the Space tree the daemon keeps; nothing
    /// here depends on what kind of Space or runtime they are.
    fn track_children(&self, rec: &AgentRecord) -> Result<()> {
        let Some(task) = rec.task.as_ref().map(|t| t.seq) else {
            return Ok(());
        };
        let kids = self.spaces.children(&rec.space).unwrap_or_default();
        // A create is told by the one member it is attributed to, on the task that member was on when it
        // saw the create (not whichever task is current when a later pass runs).
        cua_spaces::child_creates::claim(self.spaces.home_dir(), &rec.space, &rec.name, task);
        let asked: Vec<_> = cua_spaces::child_creates::of(self.spaces.home_dir(), &rec.space)
            .into_iter()
            .filter(|e| e.agent == rec.name)
            .collect();
        if kids.is_empty() && asked.is_empty() {
            return Ok(());
        }
        let store = self.thread_store(&rec.space);
        let last = store.last_seq()?;
        let recent = store.read(last.saturating_sub(RECENT * 2), RECENT as usize * 2 + 1)?;
        let infos = self.spaces.list()?;
        let mut drafts = vec![];
        // The creates the host ran for this agent: one note per step and per outcome (keyed, so a
        // pass never repeats one), which the clients fold into one row each.
        for e in &asked {
            let base = format!("childc:{}:{}", e.key, e.started_ms);
            let state = match e.state {
                cua_spaces::child_creates::State::Creating => "creating",
                cua_spaces::child_creates::State::Ready => "ready",
                cua_spaces::child_creates::State::Failed => "failed",
            };
            let key = match e.state {
                // A step posts again as it moves on (every 5 percent), so the row's bar keeps moving.
                cua_spaces::child_creates::State::Creating => format!(
                    "{base}:p:{}:{}",
                    e.phase,
                    e.percent.map_or(String::new(), |p| (p / 5 * 5).to_string())
                ),
                _ => format!("{base}:{state}"),
            };
            if recent
                .iter()
                .any(|r| r.client_id.as_deref() == Some(key.as_str()))
            {
                continue;
            }
            let os = &e.os;
            let text = match e.state {
                cua_spaces::child_creates::State::Creating => {
                    format!("Creating the {} Space {}", os, e.name).replace("  ", " ")
                }
                cua_spaces::child_creates::State::Ready => format!("{} is ready", e.name),
                cua_spaces::child_creates::State::Failed => format!(
                    "Couldn't create the {} Space {}: {}. {} was told.",
                    os,
                    e.name,
                    e.error.trim_end_matches('.').to_lowercase(),
                    rec.name
                )
                .replace("  ", " "),
            };
            let space = if e.space.is_empty() {
                format!("local:{}", e.key)
            } else {
                e.space.clone()
            };
            let mut d = Draft::event(
                "child_space",
                text,
                json!({
                    "kind": "child_space", "space": space, "key": format!("{}:{}", e.key, e.started_ms),
                    "name": e.name, "os": os, "agent": rec.handle(), "state": state,
                    "phase": e.phase, "percent": e.percent, "error": e.error,
                    "started_ms": e.started_ms, "ended_ms": e.ended_ms,
                }),
            );
            d.task_seq = Some(if e.task > 0 { e.task } else { task });
            d.client_id = Some(key);
            drafts.push(d);
        }
        let covered: std::collections::HashSet<&str> = asked
            .iter()
            .map(|e| e.space.as_str())
            .filter(|s| !s.is_empty())
            .collect();
        for id in kids {
            if covered.contains(id.as_str()) {
                continue;
            }
            let key = format!("child:{id}");
            if recent
                .iter()
                .any(|e| e.client_id.as_deref() == Some(key.as_str()))
            {
                continue;
            }
            let Some(info) = infos.iter().find(|i| i.id == id) else {
                continue;
            };
            let name = if info.display_name.is_empty() {
                info.name.clone()
            } else {
                info.display_name.clone()
            };
            let os = match info.os.as_str() {
                "macos" => "macOS",
                "windows" => "Windows",
                "linux" => "Linux",
                _ => "",
            };
            let mut d = Draft::event(
                "child_space",
                format!("{} made the computer {name}", rec.name),
                json!({"kind": "child_space", "space": id, "name": name, "os": os, "agent": rec.handle()}),
            );
            d.task_seq = Some(task);
            d.client_id = Some(key);
            drafts.push(d);
        }
        store.append_many(drafts)?;
        Ok(())
    }

    /// Follows the subagents of the session's run: absorbs the events that concern them, records
    /// each one's start and end in the thread (one note each, keyed so a pass never repeats it)
    /// and keeps the running ones for the live row. `page` is what this pass read.
    async fn track_subagents(
        &self,
        rec: &AgentRecord,
        agents: &Agents,
        run: &str,
        page: &[ag::AgentEvent],
    ) -> Result<()> {
        let key = format!("{}/{}#{}!subs", rec.space, rec.name, rec.k());
        let backfill = {
            let mut map = self.partials.lock().unwrap_or_else(|p| p.into_inner());
            let p = map.entry(key.clone()).or_default();
            if p.sub_run != run {
                *p = Partial {
                    sub_run: run.to_string(),
                    ..Partial::default()
                };
            }
            !p.backfilled
        };
        let mut earlier = vec![];
        if backfill && rec.cursor > 0 {
            // A daemon that restarted mid-run: read what concerns subagents from the start.
            let mut cursor = 0;
            loop {
                let pg = agents.events(run, cursor, 500).await?;
                let moved = pg.cursor != cursor;
                cursor = pg.cursor;
                earlier.extend(pg.events);
                if pg.caught_up || !moved || cursor >= rec.cursor {
                    break;
                }
            }
        }
        let (subs, changed) = {
            let mut map = self.partials.lock().unwrap_or_else(|p| p.into_inner());
            let p = map.entry(key.clone()).or_default();
            p.backfilled = true;
            let relevant = |e: &ag::AgentEvent| {
                e.parent_tool_id.is_some()
                    || cua_agents::subagents::is_subagent_call(e)
                    || e.kind == "exited"
                    || (e.kind == "usage"
                        && e.raw["update"]["_meta"]["_claude/origin"]["kind"].as_str()
                            == Some("task-notification"))
            };
            for e in earlier.iter().chain(page) {
                if e.seq > p.sub_last && relevant(e) {
                    p.sub_events.push(e.clone());
                }
                p.sub_last = p.sub_last.max(e.seq);
            }
            if p.sub_events.len() > 4000 {
                let drop = p.sub_events.len() - 4000;
                p.sub_events.drain(..drop);
            }
            let subs = cua_agents::subagents::fold(p.sub_events.iter());
            let changed = subs != p.subs;
            p.subs = subs.clone();
            (subs, changed)
        };
        if subs.is_empty() {
            return Ok(());
        }
        let store = self.thread_store(&rec.space);
        let last = store.last_seq()?;
        let recent = store.read(last.saturating_sub(RECENT * 2), RECENT as usize * 2 + 1)?;
        let reply_to = rec.task.as_ref().map(|t| t.seq).or_else(|| {
            store
                .fold()
                .ok()?
                .tasks
                .values()
                .filter(|t| t.to == rec.name)
                .map(|t| t.seq)
                .max()
        });
        let mut drafts = vec![];
        for s in &subs {
            // The call names its task a moment after it starts: the note waits for the name.
            if s.state == "running" && s.title == "Subagent" {
                continue;
            }
            let mut states = vec!["running"];
            if s.state != "running" {
                states.push(s.state);
            }
            for state in states {
                let id = format!("sub:{run}:{}:{state}", s.id);
                if recent
                    .iter()
                    .any(|e| e.client_id.as_deref() == Some(id.as_str()))
                {
                    continue;
                }
                let title = members::clip_line(&s.title, 120);
                let text = match state {
                    "running" => format!("{} started a subagent: {title}", rec.name),
                    "done" => format!("{}'s subagent finished: {title}", rec.name),
                    _ => format!("{}'s subagent failed: {title}", rec.name),
                };
                let result = s.result.as_deref().filter(|r| check_text(r).is_ok());
                let mut d = Draft::event(
                    "subagent",
                    text,
                    json!({
                        "kind": "subagent",
                        "id": s.id,
                        "agent": rec.handle(),
                        "title": title,
                        "state": state,
                        "steps": s.steps,
                        "result": result,
                        "started_ms": s.started_ms,
                        "ended_ms": s.ended_ms,
                    }),
                );
                d.task_seq = reply_to;
                d.client_id = Some(id);
                drafts.push(d);
            }
        }
        store.append_many(drafts)?;
        if changed {
            store.wake();
        }
        Ok(())
    }

    async fn tick_agent_inner(&self, rec: AgentRecord, report: &mut TickReport) -> Result<()> {
        let run = rec.run_id.clone().expect("filtered");
        let agents = self.cached_agents(&rec.space).await?;
        // The bridge first: an agent may be waiting on it mid-turn.
        let run_dir = agents.run_dir(&run);
        let server = bridge::server(&self.spaces, self.clone(), &rec, &run);
        let mut state = self
            .live
            .lock()
            .await
            .bridges
            .remove(&run)
            .unwrap_or_default();
        let bridged = bridge::pump(agents.guest(), &run_dir, &mut state, &server).await;
        self.live.lock().await.bridges.insert(run.clone(), state);
        report.bridged += bridged?;
        // A durable run: ship its journal, re-drive it after a crash.
        if rec.policy.durable && !rec.relaunch && !self.tick_durable(&rec, &agents, report).await? {
            return Ok(());
        }
        // Then the run's events since the bookmark.
        let page = agents.events(&run, rec.cursor, 500).await?;
        // The Space answered: the session's claims hold on (before anything
        // below can fail this pass).
        self.renew_claims(&rec, &agents, &run).await?;
        let rec = self.get(&rec.name)?.session(rec.k());
        let (ended, failed) = turn_outcome(&page.events);
        // A harness may answer a refused credential as an ordinary turn (Hermes does, with its own
        // advice): read the reply as the failed turn it is, so the person's message is not "Done",
        // the reply is the standard Connect card and the credential reads as needing a reconnect.
        let mut reply_is_the_failure = None;
        let failed = match (&failed, ended) {
            (None, Some(turn)) if turn > rec.saved_turn => agents
                .result(&run)
                .await
                .ok()
                .map(|r| r.text)
                .filter(|text| ag::failure::is_auth_failure_reply(&rec.harness, text))
                .map(|text| {
                    reply_is_the_failure = Some(turn);
                    (turn, text.trim().to_string())
                }),
            _ => failed,
        };
        // Everything the agent said, as it said it, one entry per message (the words of a turn
        // that only carried a provider's refusal are the failure, which is said in plain words
        // below). An unfinished message keeps the bookmark where it is, so it is read whole again.
        let said_open = match reply_is_the_failure {
            Some(turn) => {
                let kept: Vec<ag::AgentEvent> = page
                    .events
                    .iter()
                    .filter(|e| !(e.kind == "message" && e.turn == turn))
                    .cloned()
                    .collect();
                self.post_said(&rec, &run, &kept)?
            }
            None => self.post_said(&rec, &run, &page.events)?,
        };
        self.track_subagents(&rec, &agents, &run, &page.events)
            .await?;
        self.track_children(&rec)?;
        // The thread task this run's turn worked on: its reply and state.
        let mut anchor = None;
        let mut task_ended = false;
        if let Some(inflight) = rec.task.clone() {
            let error = failed
                .as_ref()
                .filter(|(t, _)| *t > inflight.turn)
                .map(|(_, e)| e.clone());
            if let Some(turn) = ended
                && turn > inflight.turn
            {
                let answer = match &error {
                    Some(_) => None,
                    None => Some(agents.result(&run).await?.text),
                };
                anchor = self.finish_task(&rec, &inflight, answer, error).await?;
                task_ended = true;
            } else if page.caught_up
                && let Ok(st) = agents.status(&run).await
                && st.alive == Some(false)
                && st.status != RunStatus::Idle
                && !Self::redriving(&rec, st.status)
            {
                // The run died without ending the turn (a durable one is
                // re-driven instead, and its turn goes on).
                let why = st.error.clone().unwrap_or_else(|| st.reason.clone());
                anchor = self.finish_task(&rec, &inflight, None, Some(why)).await?;
                task_ended = true;
            }
        }
        if let Some((_, err)) = &failed {
            members::note_rejected(&rec, &members::failure_for(&rec, err));
        }
        if let Some((turn, err)) = &failed {
            // One notification per failed turn, posted before anything
            // below can fail this pass (a later pass sees the same events).
            let key = format!("{run}#{turn}");
            let notify = rec.notify && rec.notified_error.as_deref() != Some(key.as_str());
            if notify {
                self.feed().post_anchored(
                    Some(&rec.name),
                    "error",
                    &notify::stopped_title(&rec.name, err),
                    err,
                    Some(&run),
                    Some(&rec.space),
                    anchor,
                )?;
                report.notified += 1;
            }
            let err = err.clone();
            self.update_in(&rec, |r| {
                r.last_error = Some(err);
                r.notified_error = Some(key);
            })?;
        }
        if task_ended && rec.policy.once {
            // `once`: its one task is done (its answer is the reply), so it
            // leaves; its home is saved first and stays.
            if let Err(e) = self.save(&rec.name).await {
                report
                    .errors
                    .push(format!("{} home not saved before it left: {e}", rec.name));
            }
            let _ = self
                .thread_store(&rec.space)
                .append(crate::thread::Draft::note(
                    "dispatch",
                    rec.task.as_ref().map(|t| t.seq),
                    format!("{} finished its task and left (added with once)", rec.name),
                ));
            self.remove(&rec.name).await?;
            return Ok(());
        }
        // What the turn is doing now, for the member's session state.
        let activity = activity_line(&page.events);
        if activity != Activity::Unchanged {
            let line = match activity {
                Activity::Line(l) => Some(l),
                _ => None,
            };
            if line != rec.activity {
                self.update_in(&rec, |r| r.activity = line)?;
                // The thread's followers show it at once.
                self.thread_store(&rec.space).wake();
            }
        }
        // A run restarted without the person asking says so once the agent
        // is up (an agent that never gets there fails instead, and says
        // that). A start the person asked for says nothing: its starting
        // row already showed it.
        if agent_ready(&page.events)
            && let Some((seq, restart)) = self.live.lock().await.joining.remove(&run)
        {
            let store = self.thread_store(&rec.space);
            let words = store
                .fold()
                .ok()
                .and_then(|f| f.tasks.get(&seq).map(|t| t.text.clone()))
                .unwrap_or_default();
            if let Some(text) = members::start_note(restart, &rec.name, &words) {
                let _ = store.append(crate::thread::Draft::note("dispatch", Some(seq), text));
            }
        }
        if failed.is_some() || ended.is_some() {
            self.live.lock().await.joining.remove(&run);
        }
        if let Some(turn) = ended
            && turn > rec.saved_turn
        {
            // Session 1 is the home's one writer; the others' turns are
            // not saved.
            let saved = if rec.k() == 1 {
                let saved = self.save(&rec.name).await?;
                report.saved.push(format!(
                    "{} turn {turn}: {} files, {} bytes, {} blocked",
                    rec.name,
                    saved.files,
                    saved.bytes,
                    saved.blocked.len()
                ));
                saved
            } else {
                home::Transfer::default()
            };
            if !saved.blocked.is_empty() {
                let what: Vec<String> = saved
                    .blocked
                    .iter()
                    .map(|(p, k)| format!("{p} ({k})"))
                    .collect();
                self.feed().post(
                    Some(&rec.name),
                    "error",
                    &format!("{} kept a secret out of its memory", rec.name),
                    &format!(
                        "Not saved to the drive: {}. Secrets belong in the Keyvault.",
                        what.join(", ")
                    ),
                    Some(&run),
                    Some(&rec.space),
                )?;
                report.notified += 1;
            }
            if rec.notify && !ended_in_error(turn, failed.as_ref()) {
                let result = agents.result(&run).await?;
                let body = if result.text.trim().is_empty() {
                    "Finished.".to_string()
                } else {
                    result.text.trim().to_string()
                };
                self.feed().post_anchored(
                    Some(&rec.name),
                    "turn_ended",
                    &rec.name,
                    &body,
                    Some(&run),
                    Some(&rec.space),
                    anchor,
                )?;
                report.notified += 1;
            }
            self.update_in(&rec, |r| r.saved_turn = turn)?;
        }
        let run_failed = failed.is_some();
        // Agent-run telemetry: the end of a run this install started.
        if (ended.is_some() || run_failed)
            && cua_telemetry::global().agent_run_pending(&run)
            && let Ok(info) = agents.status(&run).await
        {
            cua_spaces::agents::telemetry::observe(&info);
        }
        let next = if said_open && page.events.len() < 500 {
            rec.cursor
        } else {
            page.cursor
        };
        if next != rec.cursor {
            let cursor = next;
            self.update_in(&rec, |r| r.cursor = cursor)?;
        }
        if rec.k() == 1 {
            self.renew_lease(&rec.name).await?;
        }
        Ok(())
    }
}

/// The guest folder a session other than the first works in: a copy of the
/// home as last saved, next to the home (`@` never appears in an agent
/// name, so it never collides with another agent's home).
fn session_home_dir(agents: &Agents, name: &str, k: u32) -> String {
    format!("{}@{k}", agents.agent_home_dir(name))
}

/// The folder of a run that started without Cua Volume (next to the home's
/// copy on this computer, which it leaves alone).
fn offline_home_dir(agents: &Agents, name: &str, k: u32) -> String {
    match k {
        1 => format!("{}@offline", agents.agent_home_dir(name)),
        k => format!("{}@{k}-offline", agents.agent_home_dir(name)),
    }
}

/// What the person reads when `name` started without Cua Volume because
/// of `why`.
pub(crate) fn offline_notice(name: &str, why: &str) -> String {
    format!(
        "Cua Volume isn't answering on this computer right now, usually because sync with your \
         other computers is stalled. {name} started anyway, without its saved files. What it does \
         in this run stays on this computer and isn't saved to Cua Volume. Its next start uses \
         Cua Volume again once sync is back. Details: {why}"
    )
}

/// What a page of events says the turn is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Activity {
    /// Nothing new.
    Unchanged,
    /// The turn ended: nothing.
    Cleared,
    Line(String),
}

/// An MCP tool's name (`mcp__cua__stream_space_window`) in plain words for the chip, or `None` for any other title.
fn plain_tool(title: &str) -> Option<&'static str> {
    // The harness looking up which tools it has: not something the person needs to read as a tool name.
    if title == "ToolSearch" {
        return Some("getting ready");
    }
    let name = title.rsplit("__").next().unwrap_or(title);
    if !title.starts_with("mcp__") {
        return None;
    }
    Some(match name {
        "list_space_windows" => "looking at the windows",
        "stream_space_window" => "showing a window",
        "launch_space_app" => "opening an app",
        "create_space" => "making a Space",
        "delete_space" => "removing a Space",
        "start_space" | "stop_space" => "changing a Space's power",
        "list_spaces" => "looking at its Spaces",
        "space_bash" => "running a command",
        "thread_post" | "thread_reply" => "replying",
        _ => return None,
    })
}

/// The stage `events` put the turn in, in a few plain words: `installing`,
/// `starting`, `thinking`, `replying` or `using <tool>` (never the task's
/// text, never a secret). `Cleared` when the turn ended or failed after it.
fn activity_line(events: &[ag::AgentEvent]) -> Activity {
    for e in events.iter().rev() {
        let line = match e.kind {
            "turn_ended" | "exited" | "error" => return Activity::Cleared,
            "tool_call" => {
                let title = e
                    .tool_title
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .or_else(|| e.tool_kind.clone())
                    .unwrap_or_default();
                let one: String = title.split_whitespace().collect::<Vec<_>>().join(" ");
                Some(if one.is_empty() {
                    "working".to_string()
                } else if !cua_volume::scan::scan(one.as_bytes()).is_empty() {
                    "using a tool".to_string()
                } else if let Some(plain) = plain_tool(&one) {
                    plain.to_string()
                } else {
                    format!("using {}", members::clip_line(&one, 48))
                })
            }
            "message" => Some("replying".to_string()),
            "thought" | "plan" | "turn_started" | "user_message" => Some("thinking".to_string()),
            "session" | "initialized" | "authenticated" => Some("starting".to_string()),
            "install" => {
                let phase = e.raw["phase"].as_str().unwrap_or_default();
                Some(
                    if matches!(phase, "done" | "cached" | "skipped" | "ready") {
                        "starting"
                    } else {
                        "installing"
                    }
                    .to_string(),
                )
            }
            _ => None,
        };
        if let Some(l) = line {
            return Activity::Line(l);
        }
    }
    Activity::Unchanged
}

/// Whether `events` show the agent up and ready (past install and start).
fn agent_ready(events: &[ag::AgentEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e.kind, "initialized" | "session" | "turn_started"))
}

/// In a page of a run's events: the last turn that ended, and the last
/// error (its turn and message).
fn turn_outcome(events: &[ag::AgentEvent]) -> (Option<u32>, Option<(u32, String)>) {
    let ended = events
        .iter()
        .filter(|e| e.kind == "turn_ended")
        .map(|e| e.turn)
        .max();
    (ended, last_failed_turn(events))
}

/// The last turn with an error, and its first error (the cause; a later
/// one such as "the agent exited" follows from it).
fn last_failed_turn(events: &[ag::AgentEvent]) -> Option<(u32, String)> {
    let turn = events.iter().rev().find(|e| e.kind == "error")?.turn;
    failed_turns(events).into_iter().find(|(t, _)| *t == turn)
}

/// Every turn with an error, in order, each with its first error.
fn failed_turns(events: &[ag::AgentEvent]) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = vec![];
    for e in events.iter().filter(|e| e.kind == "error") {
        if out.last().is_none_or(|(t, _)| *t != e.turn) {
            out.push((e.turn, e.text.clone().unwrap_or_default()));
        }
    }
    out
}

/// Whether `turn` ended in `failed`'s error: it then gets the error's
/// notification only, not a "Finished." as well.
fn ended_in_error(turn: u32, failed: Option<&(u32, String)>) -> bool {
    failed.is_some_and(|(t, _)| *t >= turn)
}

/// Fires a routine as a turn of its agent (never interrupting one).
struct Runner(Persistent);

#[async_trait::async_trait]
impl RoutineRunner for Runner {
    async fn fire(&self, routine: &Routine) -> RoutineFiring {
        match self.0.get(&routine.bot_id) {
            Ok(rec) if rec.paused => {
                return RoutineFiring::Refused {
                    reason: format!("{} is paused", rec.name),
                };
            }
            Ok(_) => {}
            Err(e) => {
                return RoutineFiring::Failed {
                    reason: e.to_string(),
                };
            }
        }
        // A task in the agent's Space thread, delivered at once (a busy
        // agent skips the slot).
        match self.0.fire_schedule(routine).await {
            Ok(Ok(d)) => RoutineFiring::Started { run_id: d.run_id },
            Ok(Err(reason)) => RoutineFiring::Refused { reason },
            Err(Error::Agent(m)) if m.contains("middle of a turn") => {
                RoutineFiring::Refused { reason: m }
            }
            Err(e) => RoutineFiring::Failed {
                reason: e.to_string(),
            },
        }
    }
}

/// How long a local Space must be missing from the registry before the
/// agents that name it are marked gone.
const GONE_GRACE: Duration = Duration::from_secs(20);

/// How long the supervisor stays quiet about an error it already logged.
const ERROR_REPEAT: Duration = Duration::from_secs(300);

/// Logs each supervisor error once per [`ERROR_REPEAT`]: a stopped Space
/// makes every pass (about once a second) fail the same way for every agent
/// in it, which used to fill the daemon log with ~9 lines a second.
#[derive(Default)]
struct ErrorThrottle {
    seen: HashMap<String, Instant>,
}

impl ErrorThrottle {
    fn should_say(&mut self, error: &str, now: Instant) -> bool {
        if self.seen.len() > 1024 {
            self.seen
                .retain(|_, at| now.duration_since(*at) < ERROR_REPEAT);
        }
        match self.seen.get(error) {
            Some(at) if now.duration_since(*at) < ERROR_REPEAT => false,
            _ => {
                self.seen.insert(error.to_string(), now);
                true
            }
        }
    }
}

/// Persistent agents on a [`Spaces`] runtime that registered the
/// [`crate::DriveExtension`].
pub trait SpacesPersistent {
    /// The persistent agents of this runtime's cua home.
    ///
    /// # Panics
    ///
    /// When the runtime has no [`crate::DriveExtension`] (register one with
    /// [`crate::register`]).
    fn persistent(&self) -> Persistent;

    /// Runs [`Persistent::tick`] every `every` until the returned task is
    /// aborted (the daemon runs one for its lifetime).
    fn spawn_supervisor(&self, every: Duration) -> tokio::task::JoinHandle<()> {
        let p = self.persistent();
        // Members are served on their own tasks (a cold start never holds
        // up the pass), and a posted task wakes the pass at once; `every`
        // is the longest the supervisor sleeps.
        let p = p.detached();
        let wake = crate::thread::supervisor_wake();
        // The dispatcher has its own task: a message is handed to its agent
        // the moment it is posted (in this process by a wake, by another
        // process, such as the app, within half a second), whatever the
        // supervisor's pass is waiting on.
        let d = p.clone();
        let dispatch_wake = crate::thread::dispatch_wake();
        let dispatcher = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = dispatch_wake.notified() => {}
                    () = tokio::time::sleep(Duration::from_millis(500)) => {}
                }
                d.dispatch_now().await;
            }
        });
        tokio::spawn(async move {
            let _dispatcher = AbortOnDrop(dispatcher);
            let mut said = ErrorThrottle::default();
            loop {
                let r = p.tick().await;
                for e in &r.errors {
                    if said.should_say(e, Instant::now()) {
                        tracing::warn!("persistent agents: {e}");
                    }
                }
                tokio::select! {
                    () = tokio::time::sleep(every) => {}
                    () = wake.notified() => {}
                }
            }
        })
    }
}

/// Aborts its task when dropped (the supervisor's, when it is aborted).
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl SpacesPersistent for Spaces {
    fn persistent(&self) -> Persistent {
        let ext = self
            .extension::<crate::DriveExtension>()
            .expect("persistent agents need the Cua Volume extension (cua_spaces_ext::register)");
        Persistent::new(
            self.clone(),
            ext.drive().clone(),
            ext.live(),
            ext.partials(),
            ext.owner(),
            ext.team_admins(),
        )
    }
}

/// Parses a routine schedule from the tool arguments: exactly one of
/// `every_minutes`, `daily_at` (`HH:MM`) or `weekly_on` (`<weekday> HH:MM`).
pub fn parse_schedule(
    every_minutes: Option<i64>,
    daily_at: Option<&str>,
    weekly_on: Option<&str>,
) -> Result<Schedule> {
    let hm = |s: &str| -> Result<(u32, u32)> {
        let (h, m) = s
            .trim()
            .split_once(':')
            .ok_or_else(|| Error::invalid(format!("{s:?}: use HH:MM")))?;
        let (h, m): (u32, u32) = (
            h.parse()
                .map_err(|_| Error::invalid(format!("{s:?}: use HH:MM")))?,
            m.parse()
                .map_err(|_| Error::invalid(format!("{s:?}: use HH:MM")))?,
        );
        if h > 23 || m > 59 {
            return Err(Error::invalid(format!("{s:?}: out of range")));
        }
        Ok((h, m))
    };
    match (every_minutes, daily_at, weekly_on) {
        (Some(m), None, None) if m > 0 => Ok(Schedule::EveryMinutes { minutes: m }),
        (Some(_), None, None) => Err(Error::invalid("every_minutes must be at least 1")),
        (None, Some(t), None) => {
            let (hour, minute) = hm(t)?;
            Ok(Schedule::DailyAt { hour, minute })
        }
        (None, None, Some(w)) => {
            let (day, t) = w
                .trim()
                .split_once(' ')
                .ok_or_else(|| Error::invalid(format!("{w:?}: use `<weekday> HH:MM`")))?;
            let days = [
                "sunday",
                "monday",
                "tuesday",
                "wednesday",
                "thursday",
                "friday",
                "saturday",
            ];
            let d = day.to_ascii_lowercase();
            let weekday = days
                .iter()
                .position(|x| x.starts_with(&d) && d.len() >= 3)
                .ok_or_else(|| Error::invalid(format!("{day:?}: not a weekday")))?
                as u32
                + 1;
            let (hour, minute) = hm(t)?;
            Ok(Schedule::WeeklyOn {
                weekday,
                hour,
                minute,
            })
        }
        _ => Err(Error::invalid(
            "give exactly one of every_minutes, daily_at or weekly_on",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_supervisor_error_is_logged_once_per_window() {
        let mut t = ErrorThrottle::default();
        let t0 = Instant::now();
        assert!(t.should_say("sandbox a is Stopped", t0));
        assert!(!t.should_say("sandbox a is Stopped", t0 + Duration::from_secs(1)));
        assert!(t.should_say("sandbox b is Stopped", t0 + Duration::from_secs(1)));
        assert!(!t.should_say(
            "sandbox a is Stopped",
            t0 + ERROR_REPEAT - Duration::from_secs(1)
        ));
        assert!(t.should_say("sandbox a is Stopped", t0 + ERROR_REPEAT));
    }

    // An events.jsonl line as the runner writes it.
    fn ev(kind: &str, turn: u32, text: Option<&str>) -> ag::AgentEvent {
        let line =
            serde_json::json!({"seq": 1, "ts": 0, "turn": turn, "type": kind, "message": text});
        ag::AgentEvent::parse(&line.to_string()).unwrap()
    }

    #[test]
    fn a_run_in_another_home_is_not_in_this_one() {
        // The run started on the mounted Volume; the Volume is not the
        // agent's now (or the home is copied in): its folders are not there.
        let vol = "/volume/agents/claude-2";
        assert!(Persistent::in_home("/volume/agents/claude-2/work", vol));
        assert!(Persistent::in_home(vol, "/volume/agents/claude-2/"));
        assert!(!Persistent::in_home(
            "/volume/agents/claude-2/work",
            "/home/cua/.cua/homes/claude-2"
        ));
        assert!(!Persistent::in_home("/volume/agents/claude-22/work", vol));
    }

    #[test]
    fn a_failed_turn_is_one_error_not_finished_plus_an_error() {
        // The runner's failed turn: an error, then turn_ended (stop: error).
        let evs = [
            ev("turn_started", 1, None),
            ev("error", 1, Some("Authentication required")),
            ev("turn_ended", 1, None),
        ];
        let (ended, failed) = turn_outcome(&evs);
        assert_eq!(ended, Some(1));
        assert_eq!(failed, Some((1, "Authentication required".into())));
        assert!(ended_in_error(1, failed.as_ref()));
        // An earlier turn's error does not hide a later turn's result.
        let evs = [
            ev("error", 1, Some("x")),
            ev("turn_ended", 1, None),
            ev("turn_ended", 2, None),
        ];
        let (ended, failed) = turn_outcome(&evs);
        assert!(!ended_in_error(ended.unwrap(), failed.as_ref()));
        let (ended, failed) = turn_outcome(&[ev("turn_ended", 1, None)]);
        assert!(!ended_in_error(ended.unwrap(), failed.as_ref()));
    }

    #[test]
    fn schedules_parse() {
        assert_eq!(
            parse_schedule(Some(30), None, None).unwrap(),
            Schedule::EveryMinutes { minutes: 30 }
        );
        assert_eq!(
            parse_schedule(None, Some("08:05"), None).unwrap(),
            Schedule::DailyAt { hour: 8, minute: 5 }
        );
        assert_eq!(
            parse_schedule(None, None, Some("Mon 09:30")).unwrap(),
            Schedule::WeeklyOn {
                weekday: 2,
                hour: 9,
                minute: 30
            }
        );
        for bad in [
            parse_schedule(None, None, None),
            parse_schedule(Some(0), None, None),
            parse_schedule(Some(5), Some("08:00"), None),
            parse_schedule(None, Some("25:00"), None),
            parse_schedule(None, None, Some("Funday 09:00")),
        ] {
            assert_eq!(bad.unwrap_err().tag(), "invalid_argument");
        }
    }
}

/// Whether a Space in this power state is off for the supervisor: stopped,
/// or suspended (#5912), which counts as stopped. A durable member never
/// wakes it; it waits until the Space runs again.
pub(crate) fn space_is_off(power_state: &str) -> bool {
    matches!(power_state, "stopped" | "suspended")
}

#[cfg(test)]
mod power_tests {
    use super::space_is_off;

    #[test]
    fn suspended_counts_as_stopped_and_running_or_unknown_does_not() {
        assert!(space_is_off("stopped"));
        assert!(space_is_off("suspended"));
        assert!(!space_is_off("running"));
        assert!(!space_is_off(""));
    }
}

/// Whether a stretch of an agent's words is narration of its own process ("Let me look at the
/// logo", "I'll check the packages", "Now I'm writing the script"): the agent speaks as the
/// person's computers, and narrates neither tools nor plans. Held back in the thread; the app's own
/// working line shows that it is busy.
fn is_process_narration(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    let first: String = t
        .split(['.', '!', '\n'])
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    // "Got the logo. Let me look at it": any sentence that starts a plan counts.
    let sentences: Vec<String> = t
        .split(['.', '!', '\n'])
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect();
    const STARTS: [&str; 12] = [
        "let me ",
        "let's ",
        "i'll ",
        "i will ",
        "i'm going to ",
        "i am going to ",
        "now i",
        "next, i",
        "first, i",
        "i need to ",
        "i'm checking",
        "i'm looking",
    ];
    let narrates = |x: &str| STARTS.iter().any(|p| x.starts_with(p));
    narrates(&first) || (sentences.len() <= 3 && sentences.iter().any(|x| narrates(x)))
}

#[cfg(test)]
mod voice_tests {
    use super::is_process_narration;

    #[test]
    fn narration_of_the_process_is_held_back_and_a_final_message_is_not() {
        for n in [
            "Got the logo. Let me look at it and see which packages exist for rasterizing.",
            "I'll check the pool skill first.",
            "Now I'm writing the training script.",
            "Let's try a longer run.",
        ] {
            assert!(is_process_narration(n), "{n}");
        }
        for ok in [
            "Training on your Mac with MLX, about 90 seconds.",
            "Your Cua logo has grown from a single cell. The window is open in your Stream: click it to erase a patch and watch it regrow.",
            "Opening the window now.",
        ] {
            assert!(!is_process_narration(ok), "{ok}");
        }
    }
}

#[cfg(test)]
mod plain_tool_tests {
    use super::plain_tool;

    #[test]
    fn the_chip_names_what_the_agent_does_not_the_tool() {
        assert_eq!(
            plain_tool("mcp__cua__stream_space_window"),
            Some("showing a window")
        );
        assert_eq!(
            plain_tool("mcp__cua__launch_space_app"),
            Some("opening an app")
        );
        assert_eq!(plain_tool("ToolSearch"), Some("getting ready"));
        assert_eq!(plain_tool("Terminal"), None);
        assert_eq!(plain_tool("mcp__other__thing"), None);
    }
}
