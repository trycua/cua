// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Agent members and the Space thread: a persistent agent is an agent
//! member of the Space it works in, and the Space's thread
//! ([`crate::thread`]) is where people and members talk.
//!
//! - [`Thread`]: posting (authors stamped from the caller, @mentions of
//!   members become tasks, attachments go through the Volume), reading with
//!   per-member cursors, the inbox, claims and replies, and imports.
//! - The dispatcher ([`Persistent::dispatch`], in the supervisor tick):
//!   the oldest queued task of a member that is not mid-turn and not paused
//!   is delivered as its next turn ([`Persistent::send_from`] restores the
//!   home and wakes an asleep agent), recorded `claimed` then `working`; when
//!   that turn ends, the turn's answer is posted as the member's reply
//!   unless it replied itself, and the task is `done` (`failed` on an error).
//! - Schedules (routines) post a task authored `system:schedule:<id>` and
//!   go through the same delivery, keeping the skip-if-busy rule.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{AgentRecord, AgentSpec, MAX_SESSIONS, Persistent, locked_json};
use crate::DriveResult as _;
use crate::thread::claims::{ClaimEntries, Holder};
use crate::thread::policy::{self, Right, Rights};
use crate::thread::{
    Answer, Approval, Attachment, Author, Draft, EntryKind, Import, MAX_ATTACHMENTS, TaskInfo,
    TaskState, TaskTo, ThreadEntry, ThreadStore, Update, check_text, new_entry_id, parse_mentions,
    refuse_secrets,
};
use cua_agents::RunStatus;
use cua_spaces::agents as ag;
use cua_spaces::extension::Caller;
use cua_spaces::{Error, Result};

/// How long a task claim lasts without renewal. The supervisor renews the
/// claims of a session whose run is alive and working on them, so a claim
/// runs out only when its session is gone.
/// The id of the desktop row of this machine ("This Mac"). It is not a Space,
/// but its page has the thread of any Space: people write there, and nothing
/// runs there (no agent is a member of it). Its thread lives on this machine
/// only: it is not mirrored to the Volume, so it never mixes with another
/// machine's.
pub const HOST_DESKTOP: &str = "this-mac";

pub const CLAIM_TTL: std::time::Duration = std::time::Duration::from_secs(120);

/// The largest local file attached to a post.
pub const MAX_LOCAL_ATTACHMENT: u64 = 100 * 1024 * 1024;
/// Most entries one `thread_import` call takes.
pub const MAX_IMPORT: usize = 5000;

/// How an agent member runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    /// Re-drive its runs after a crash.
    #[serde(default)]
    pub durable: bool,
    /// `local`, `auto` or a machine.
    #[serde(default = "local")]
    pub on: String,
    /// Removed when its first task ends.
    #[serde(default)]
    pub once: bool,
    /// Guard mode for durable runs.
    #[serde(default = "super::yes")]
    pub guard: bool,
    /// Who can task, control and approve it (absent: the defaults,
    /// [`Rights::default`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rights: Option<Rights>,
}

fn local() -> String {
    "local".into()
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            durable: false,
            on: local(),
            once: false,
            guard: true,
            rights: None,
        }
    }
}

/// The task a member's current turn works on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InFlight {
    /// The task's entry.
    pub seq: u64,
    /// The run's turn before the task was delivered: a later `turn_ended`
    /// ends the task.
    pub turn: u32,
}

/// What a member was added with besides its [`AgentSpec`].
#[derive(Clone, Debug, Default)]
pub struct MemberOptions {
    pub durable: bool,
    pub on: Option<String>,
    pub once: bool,
    pub sessions: Option<u32>,
    /// Who can task, control and approve it (`None`: the defaults).
    pub rights: Option<Rights>,
    /// `on: auto`: the durable Space (by name) `spec.space` is now.
    pub durable_space: Option<String>,
}

/// What `thread_post` posts.
#[derive(Clone, Debug, Default)]
pub struct PostArgs {
    pub text: String,
    pub reply_to: Option<u64>,
    /// Volume paths, or local paths for this machine's own user.
    pub attachments: Vec<String>,
    /// `(action, details)`.
    pub approval: Option<(String, String)>,
    /// `(approval seq, approve, note)`.
    pub answer: Option<(u64, bool, String)>,
    /// The session of the mentioned agent that should take the task
    /// (default: any free session).
    pub to_session: Option<u32>,
    /// What the person was looking at in the Space (its desktop or a
    /// window), stored with the entry.
    pub view: Option<crate::thread::ViewContext>,
}

/// One entry of an import.
#[derive(Clone, Debug, Default)]
pub struct ImportEntry {
    pub import_id: String,
    pub ts_ms: u64,
    /// `person` or `agent`.
    pub author_kind: String,
    pub agent_name: Option<String>,
    pub text: String,
    /// `message`, `approval` or `answer`.
    pub kind: Option<String>,
}

fn denied(msg: impl Into<String>) -> Error {
    Error::extension("permission_denied", msg)
}

/// Managing members (add, remove, schedules) is the owner's.
pub fn may_manage(caller: &Caller) -> Result<()> {
    match caller {
        Caller::Owner => Ok(()),
        Caller::Relay { role, .. } if role == "owner" => Ok(()),
        _ => Err(denied("only the owner of this machine manages its agents")),
    }
}

fn mime_of(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "csv" => "text/csv",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

/// A path on this machine (absolute, `~/`, `./` or `../`) rather than a
/// Cua Volume path.
fn is_local_path(p: &str) -> bool {
    p.starts_with('/') || p.starts_with("~/") || p.starts_with("./") || p.starts_with("../")
}

fn expand_home(p: &str) -> std::path::PathBuf {
    if let Some(rest) = p.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return Path::new(&home).join(rest);
    }
    std::path::PathBuf::from(p)
}

impl Persistent {
    /// This machine's user as a thread author.
    pub fn owner(&self) -> Author {
        self.owner
            .read()
            .map(|o| o.clone())
            .unwrap_or_else(|_| Author::owner())
    }

    /// The author the daemon stamps for `caller`.
    pub fn author_for(&self, caller: &Caller) -> Author {
        match caller {
            Caller::Owner => self.owner(),
            Caller::Relay { account, name, .. } => Author {
                kind: crate::thread::AuthorKind::Person,
                id: account.clone(),
                name: name.clone().unwrap_or_else(|| "Guest".into()),
                identity: None,
            },
            Caller::Agent { name, .. } => Author::agent(name),
            Caller::Member { handle, .. } => Author::agent(handle),
        }
    }

    /// The thread of `space` (a Space id as recorded), without checks.
    pub fn thread_store(&self, space: &str) -> ThreadStore {
        ThreadStore::open(self.spaces.home_dir(), space)
    }

    /// The thread key of `space`: its Space id, or [`HOST_DESKTOP`] for the
    /// desktop row of this machine (no Space, but a page with a thread).
    fn thread_key(&self, space: &str) -> Result<String> {
        if space == HOST_DESKTOP {
            return Ok(HOST_DESKTOP.to_string());
        }
        Ok(self.spaces.resolve(space)?.to_string())
    }

    /// The thread of `space` (id or name) for `caller`: an agent reaches
    /// only its own Space's. A thread with no local log is rebuilt from the
    /// Volume first (once per process).
    pub async fn thread(&self, caller: &Caller, space: &str) -> Result<Thread> {
        // Everyone the transport let in reads (guests too); writing is
        // checked where each entry is written ([`policy`]).
        let id = self.thread_key(space)?;
        if let Some((_, own)) = caller.agent_member()
            && *own != id
        {
            return Err(denied("an agent posts only in its own Space's thread"));
        }
        let store = self.thread_store(&id);
        // This machine's desktop thread stays on this machine: another
        // machine's "this-mac" is not the same place.
        if id != HOST_DESKTOP {
            self.rebuild_once(&store).await;
        }
        Ok(Thread {
            p: self.clone(),
            store,
            caller: caller.clone(),
            session: None,
        })
    }

    /// Rebuilds `store` from the Volume when it has no local log, once per
    /// process (bounded; a failure leaves an empty thread).
    pub(crate) async fn rebuild_once(&self, store: &ThreadStore) {
        if store.exists() {
            return;
        }
        if !self
            .live
            .lock()
            .await
            .rebuilt
            .insert(store.space().to_string())
        {
            return;
        }
        let session = self.drive.session(cua_volume::Context::user());
        match tokio::time::timeout(
            std::time::Duration::from_secs(10),
            crate::thread::mirror::rebuild(store, &session),
        )
        .await
        {
            Ok(Ok(n)) if n > 0 => {
                tracing::info!(
                    space = store.space(),
                    entries = n,
                    "thread rebuilt from the volume"
                )
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::warn!(space = store.space(), "thread not rebuilt: {e}"),
            Err(_) => tracing::warn!(space = store.space(), "thread not rebuilt: timed out"),
        }
    }

    /// The agent members of `space` (a Space id).
    pub fn members_of(&self, space: &str) -> Result<Vec<AgentRecord>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|r| r.space == space)
            .collect())
    }

    /// A Space of this machine that is no longer there: its members' keys are
    /// free again. Only the registry of this machine answers for itself, for a
    /// local Space or an address added by hand (a Space on another computer
    /// may be offline, not gone).
    fn space_is_gone(&self, space: &str) -> bool {
        (space.starts_with("local:") || space.starts_with("direct:"))
            && self
                .spaces
                .list()
                .map(|rows| !rows.iter().any(|s| s.id == space))
                .unwrap_or(false)
    }

    /// `base`, or `base-2`, `base-3`, ... : the first key no persistent agent has.
    fn free_key(&self, base: &str) -> Result<String> {
        let taken: Vec<String> = self.list()?.into_iter().map(|r| r.name).collect();
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|k| k.len() <= 63 && !taken.contains(k))
            .ok_or_else(|| Error::invalid(format!("no free name for {base}")))
    }

    /// Adds an agent member: a persistent agent of `spec.space`, added by
    /// `caller`, with its policy (`durable`, `on`, `once`) and 1 to
    /// [`MAX_SESSIONS`] sessions. Its Space's `.cua/setup` runs before it
    /// gets a task. A member of that name that exists already in the Space
    /// (same harness, for example one an older Cua created) takes the new
    /// policy. A name used by another Space does not matter: each Space's
    /// `@claude` is its own.
    pub fn add_member(
        &self,
        caller: &Caller,
        spec: AgentSpec,
        opts: MemberOptions,
    ) -> Result<AgentRecord> {
        may_manage(caller)?;
        let on = opts.on.clone().unwrap_or_else(local);
        if opts.sessions.is_some_and(|n| n == 0 || n > MAX_SESSIONS) {
            return Err(Error::invalid(format!("sessions: 1 to {MAX_SESSIONS}")));
        }
        if opts.sessions.is_some_and(|n| n == 0 || n > MAX_SESSIONS) {
            return Err(Error::invalid(format!("sessions: 1 to {MAX_SESSIONS}")));
        }
        if on == "auto" && opts.durable_space.is_none() {
            return Err(Error::invalid(
                "on: auto needs the member's Space to be a durable Space",
            ));
        }
        let space = self.spaces.resolve(&spec.space)?.to_string();
        let wanted = spec.name.clone();
        // This Space's own member of that name (an older Cua's included).
        let here = self
            .members_of(&space)?
            .into_iter()
            .find(|r| r.handle() == wanted);
        let rec = match here {
            Some(r) => {
                if r.harness != spec.harness {
                    return Err(Error::invalid(format!(
                        "{wanted} in this Space is {}, not {}: pick another name",
                        r.harness, spec.harness
                    )));
                }
                r
            }
            None => {
                // Names are keys on this machine, but a Space's `@claude` is its
                // own: when the key is taken elsewhere (or by a Space that is
                // gone, which frees it), this Space's member gets a key of its
                // own and answers to the name it was asked for.
                let mut spec = spec;
                if let Ok(other) = self.get(&wanted) {
                    if self.space_is_gone(&other.space) {
                        locked_json(&self.agents_path(), |all: &mut Vec<AgentRecord>| {
                            all.retain(|a| a.name != other.name)
                        })?;
                    } else {
                        spec.name = self.free_key(&wanted)?;
                    }
                }
                let alias = (spec.name != wanted).then(|| wanted.clone());
                let rec = self.create(spec)?;
                if alias.is_some() {
                    self.update(&rec.name, |r| r.handle = alias.clone())?;
                }
                self.get(&rec.name)?
            }
        };
        let added_by = self.author_for(caller);
        self.update(&rec.name, |r| {
            if r.added_by.is_none() {
                r.added_by = Some(added_by);
            }
            r.policy = Policy {
                durable: opts.durable,
                on,
                once: opts.once,
                guard: true,
                rights: opts.rights.clone(),
            };
            r.durable_space = opts.durable_space.clone();
            r.sessions = opts.sessions.unwrap_or(1);
            r.setup = super::durable::setup::PENDING.into();
            r.setup_note.clear();
        })?;
        let rec = self.get(&rec.name)?;
        self.prewarm_in_background(&rec);
        Ok(rec)
    }

    /// Puts the member's agent stack in its Space now, in the background,
    /// so its first message does not wait for the install. A start that
    /// begins meanwhile waits for this instead of installing twice; a
    /// Space that cannot be reached yet is left for the start.
    fn prewarm_in_background(&self, rec: &AgentRecord) {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        if super::identity::is_external(rec) {
            return;
        }
        let me = self.clone();
        let rec = rec.clone();
        rt.spawn(async move {
            let Some(h) = cua_agents::harness::harness(&rec.harness).filter(|h| h.ready) else {
                return;
            };
            let lock = super::phases::prewarm_lock(&me.dir, &rec.space, &rec.harness);
            let _held = lock.lock().await;
            let Ok(agents) = me.agents_for(&rec.space).await else {
                return;
            };
            let _ = agents.prewarm(h, true).await;
            let install = agents.ensure(h.installs, |_| {});
            match tokio::time::timeout(std::time::Duration::from_secs(600), install).await {
                Ok(Ok(_)) => tracing::debug!(agent = %rec.name, "agent stack ready ahead of its first message"),
                Ok(Err(e)) => tracing::debug!(agent = %rec.name, "prewarm install: {e}"),
                Err(_) => tracing::debug!(agent = %rec.name, "prewarm install timed out"),
            }
        });
    }

    /// The agent members of `space` with what each is doing: the member's
    /// `status` (`working | waiting | failed | idle | asleep | paused`, the
    /// busiest of its sessions) and each session's in `session_states`.
    pub async fn members(&self, space: &str) -> Result<Vec<Value>> {
        let id = self.thread_key(space)?;
        let recs = self.members_of(&id)?;
        let store = self.thread_store(&id);
        let fold = store.fold()?;
        let mut out = vec![];
        let live = |r: &AgentRecord| r.session_views().iter().any(|s| s.run_id.is_some());
        let agents = if recs.iter().any(|r| live(r) && !r.paused) {
            match tokio::time::timeout(std::time::Duration::from_secs(15), self.cached_agents(&id))
                .await
            {
                Ok(Ok(a)) => Some(a),
                _ => None,
            }
        } else {
            None
        };
        for r in recs {
            let open = fold.open_tasks(Some(&r.name)).len();
            let mut sessions = vec![];
            let mut first_status = None;
            for s in r.session_views() {
                let mut run_status = None;
                if let (Some(run), Some(a), false) = (&s.run_id, &agents, r.paused) {
                    run_status = a.status(run).await.ok().map(|st| st.status);
                }
                if s.k() <= 1 {
                    first_status = run_status;
                }
                sessions.push(session_state(&s, run_status, &fold));
            }
            // Session 1's run (a durable re-drive is session 1's).
            let resuming = first_status.is_some_and(|s| Persistent::redriving(&r, s));
            let held = r.held_by.as_ref().map(|a| a.name.clone());
            let external = super::identity::is_external(&r);
            let state = if external && r.identity.as_ref().is_some_and(|i| i.pending) {
                "pending"
            } else if held.is_some() {
                "paused"
            } else if !r.paused && (resuming || r.relaunch) {
                // A durable run being resumed after a crash.
                "working"
            } else if !r.paused && r.setup == super::durable::setup::FAILED {
                "failed"
            } else {
                member_state(r.paused, &sessions)
            };
            let activity = match state {
                "paused" if held.is_some() => held.map(|n| format!("{n} has control")),
                "working" => sessions
                    .iter()
                    .find(|s| s.state == "working")
                    .and_then(|s| s.task_seq)
                    .and_then(|seq| fold.tasks.get(&seq))
                    .map(|t| format!("task #{}: {}", t.seq, clip_words(&t.text, 80))),
                "failed" if r.setup == super::durable::setup::FAILED => {
                    Some(super::notify::reason(&r.setup_note))
                }
                "waiting" => sessions
                    .iter()
                    .find(|s| s.state == "waiting")
                    .and_then(|s| s.activity.clone()),
                "failed" => sessions
                    .iter()
                    .find(|s| s.state == "failed")
                    .and_then(|s| s.activity.clone())
                    .or_else(|| r.last_error.as_deref().map(|e| failed_line(&r.harness, e))),
                "idle" | "asleep" if open > 0 => Some(format!(
                    "{open} open task{}",
                    if open == 1 { "" } else { "s" }
                )),
                _ => None,
            };
            let mut status = json!({"state": state});
            if let Some(a) = activity {
                status["activity"] = json!(a);
            }
            // The typed failure (and its complete text) of a failed member.
            if state == "failed"
                && let Some(f) = sessions
                    .iter()
                    .find_map(|s| s.failure.clone())
                    .or_else(|| r.last_error.as_deref().map(|e| failure_for(&r, e)))
            {
                status["failure"] = json!(f);
            }
            let mut row = member_row(&r, &self.owner());
            row["run_id"] = json!(r.run_id);
            row["status"] = status;
            row["open_tasks"] = json!(open);
            row["session_states"] = json!(sessions);
            out.push(row);
        }
        with_row_collisions(&mut out);
        Ok(out)
    }

    // --- delivery -------------------------------------------------------

    /// The supervisor's dispatcher: requeues tasks whose claim ran out,
    /// then gives each member's queued tasks, oldest first, to its free
    /// sessions (the session a task names, else the lowest free one).
    pub(crate) async fn dispatch(&self, report: &mut super::TickReport) {
        let records = match self.list() {
            Ok(r) => r,
            Err(e) => {
                report.errors.push(format!("dispatch: {e}"));
                return;
            }
        };
        // A member someone took control of gets nothing until control is
        // given back. A connected agent takes its tasks from its inbox
        // itself. A member waiting for (or failed by) its .cua/setup, or
        // whose durable run is being started again elsewhere, keeps its tasks.
        let members: Vec<AgentRecord> = records
            .into_iter()
            .filter(|r| {
                !r.paused
                    && r.held_by.is_none()
                    && !super::identity::is_external(r)
                    && !r.relaunch
                    && r.setup != super::durable::setup::PENDING
                    && r.setup != super::durable::setup::FAILED
            })
            .filter(|r| self.thread_store(&r.space).exists())
            .collect();
        // Each member is served on its own, so one member's cold start
        // (minutes, on first use) never holds up another member's reply.
        if self.detach_dispatch {
            for rec in members {
                let Some(guard) = super::phases::ServingGuard::take(&self.dir, &rec.name) else {
                    continue; // still being served by an earlier pass
                };
                let me = self.clone();
                tokio::spawn(async move {
                    let _guard = guard;
                    let store = me.thread_store(&rec.space);
                    match tokio::time::timeout(
                        super::DISPATCH_BUDGET,
                        me.dispatch_member(&rec, &store),
                    )
                    .await
                    {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => tracing::warn!("{} dispatch: {e}", rec.name),
                        Err(_) => tracing::warn!("{} dispatch: timed out", rec.name),
                    }
                });
            }
            return;
        }
        let results = futures_util::future::join_all(members.iter().map(|rec| async move {
            let store = self.thread_store(&rec.space);
            let r = tokio::time::timeout(super::DISPATCH_BUDGET, self.dispatch_member(rec, &store))
                .await;
            (rec.name.as_str(), r)
        }))
        .await;
        for (name, r) in results {
            match r {
                Ok(Ok(seqs)) => {
                    for seq in seqs {
                        report.fired.push(format!("{name}: task #{seq}"));
                    }
                }
                Ok(Err(e)) => report.errors.push(format!("{name} dispatch: {e}")),
                Err(_) => report.errors.push(format!("{name} dispatch: timed out")),
            }
        }
    }

    async fn dispatch_member(&self, rec: &AgentRecord, store: &ThreadStore) -> Result<Vec<u64>> {
        // A claim nobody renewed (its session is gone or unreachable): the
        // task goes back to the queue, and the session lets go of it.
        let requeued = store.requeue_expired(&rec.name, &[])?;
        let mut sessions = rec.session_views();
        for s in &mut sessions {
            if s.task.as_ref().is_some_and(|t| requeued.contains(&t.seq)) {
                self.update_in(s, |r| r.task = None)?;
                s.task = None;
            }
        }
        let mut free: Vec<AgentRecord> =
            sessions.into_iter().filter(|s| s.task.is_none()).collect();
        let mut out = vec![];
        let queued: Vec<TaskInfo> = store
            .fold()?
            .open_tasks(Some(&rec.name))
            .into_iter()
            .filter(|t| t.state == TaskState::Queued)
            .collect();
        for task in queued {
            if free.is_empty() {
                break;
            }
            let mut i = 0;
            while i < free.len() {
                if task.to_session.is_some_and(|k| k != free[i].k()) {
                    i += 1;
                    continue;
                }
                match self.deliver(&free[i], store, &task, "thread").await? {
                    Delivered::Yes(_) => {
                        free.remove(i);
                        out.push(task.seq);
                        break;
                    }
                    // Mid turn: this session is not free after all.
                    Delivered::Busy => {
                        free.remove(i);
                    }
                    // Someone claimed it meanwhile.
                    Delivered::Taken => break,
                }
            }
        }
        Ok(out)
    }

    /// Delivers `task` to the session `rec` shows as its next turn: claims
    /// the task for the session (exactly one session gets it), then sends
    /// it. A delivery that fails marks the task failed with the reason.
    pub(crate) async fn deliver(
        &self,
        rec: &AgentRecord,
        store: &ThreadStore,
        task: &TaskInfo,
        entry: &str,
    ) -> Result<Delivered> {
        let picked_ms = super::phases::now_ms();
        let (base, cold) = match self.current_turn(rec).await {
            Some(Some(turn)) => (turn, false),
            Some(None) => return Ok(Delivered::Busy),
            None => (0, true),
        };
        let holder = Holder::new(&rec.name, rec.k(), rec.drive_epoch());
        let dispatch = || ClaimEntries {
            states: vec![TaskState::Claimed],
            author: Some(Author::system("dispatch")),
            note: String::new(),
        };
        match store.claim(task.seq, &holder, claim_ttl_ms(), dispatch()) {
            Ok(_) => {}
            Err(e) if e.tag() == crate::thread::claims::TASK_CLAIMED => {
                return Ok(Delivered::Taken);
            }
            Err(e) => return Err(e),
        }
        let posted = store.get(task.seq)?;
        let text = posted
            .as_ref()
            .map(|e| e.text.clone())
            .unwrap_or_else(|| task.text.clone());
        let framed = format!(
            "[thread #{} from {}] {text}\n\n(Your answer is posted as your reply in the Space's thread when this turn ends; the `cua` tools thread_reply, thread_post and thread_read reach the thread yourself.)",
            task.seq, task.author.name
        );
        let update = |state, note: String| {
            Draft::update(
                Author::system("dispatch"),
                Update {
                    task: task.seq,
                    state,
                    session: None,
                    lease_until_ms: None,
                    note,
                },
            )
        };
        // How long the join takes is measured from the message.
        let posted_ms = posted.as_ref().map(|e| e.ts_ms).unwrap_or(picked_ms);
        let woke = super::phases::Woke {
            posted_ms,
            picked_ms,
            store: store.clone(),
            task: task.seq,
            label: session_name(rec),
        };
        if cold {
            // Said at once; "woke up and joined" follows when the agent has
            // a session (the start itself is quick, the install is not).
            store.append(Draft::note(
                "dispatch",
                Some(task.seq),
                format!("{} is joining...", session_name(rec)),
            ))?;
            super::phases::set_woke(
                &self.dir,
                &format!("{}#{}", rec.name, rec.k()),
                woke.clone(),
            );
        }
        match Box::pin(self.send_session(rec, &framed, entry)).await {
            Ok(d) => {
                let base = if d.started { 0 } else { base };
                if !d.started {
                    // A follow-up to a running agent: its steps are timed too.
                    Box::pin(self.follow_warm(rec, &d.run_id, posted_ms, picked_ms)).await;
                } else if !cold {
                    // The run was ended and started again (new credentials):
                    // nothing said it was joining, so say it has.
                    store.append(Draft::note(
                        "dispatch",
                        Some(task.seq),
                        format!("{} woke up and joined", session_name(rec)),
                    ))?;
                }
                store.claim(
                    task.seq,
                    &holder,
                    claim_ttl_ms(),
                    ClaimEntries {
                        states: vec![TaskState::Working],
                        ..dispatch()
                    },
                )?;
                self.update_in(rec, |r| {
                    r.task = Some(InFlight {
                        seq: task.seq,
                        turn: base,
                    })
                })?;
                Ok(Delivered::Yes(d))
            }
            Err(Error::Agent(m)) if m.contains("middle of a turn") => {
                store.release(task.seq, vec![update(TaskState::Queued, String::new())])?;
                Ok(Delivered::Busy)
            }
            Err(e) => {
                super::phases::take_woke(&self.dir, &format!("{}#{}", rec.name, rec.k()));
                if cold {
                    super::phases::start_failed(
                        &format!("{}/{}", rec.name, task.seq),
                        &rec.harness,
                        &self.runtime_word(&rec.space),
                        posted_ms,
                        &e.to_string(),
                    );
                }
                // The agent answers in the thread: plain words, with the
                // complete error (and a Connect button when it needs a
                // sign-in) in `failure`.
                let failure = failure_for(rec, &e.to_string());
                let mut reply = Draft::new(
                    Author::agent(&rec.name),
                    EntryKind::Message,
                    ag::failure::plain(&failure, &rec.harness),
                );
                reply.reply_to = Some(task.seq);
                reply.failure = Some(failure);
                store.release(
                    task.seq,
                    vec![
                        reply,
                        update(
                            TaskState::Failed,
                            format!("could not start {}", session_name(rec)),
                        ),
                    ],
                )?;
                Err(e)
            }
        }
    }

    /// Starts timing a follow-up to a running agent (`cua_agent_join_phase`).
    async fn follow_warm(&self, rec: &AgentRecord, run_id: &str, posted_ms: u64, picked_ms: u64) {
        if let Ok(agents) = self.cached_agents(&rec.space).await {
            super::phases::follow(
                agents,
                super::phases::Join {
                    run_id: run_id.to_string(),
                    harness: rec.harness.clone(),
                    runtime: self.runtime_word(&rec.space),
                    cold: false,
                    posted_ms,
                },
                vec![
                    ("posted", 0),
                    ("dispatch_picked", picked_ms.saturating_sub(posted_ms)),
                ],
                0,
                None,
            );
        }
    }

    /// The turn of `rec`'s run when it can take a message: `None` (no
    /// run), `Some(None)` (mid turn), `Some(Some(turn))`.
    async fn current_turn(&self, rec: &AgentRecord) -> Option<Option<u32>> {
        let run = rec.run_id.as_ref()?;
        let agents = self.cached_agents(&rec.space).await.ok()?;
        match agents.status(run).await {
            Ok(st) if st.status == RunStatus::Running => Some(None),
            // A durable run being re-driven: its turn goes on.
            Ok(st) if Persistent::redriving(rec, st.status) => Some(None),
            Ok(st) => Some(Some(st.turn)),
            Err(_) => None,
        }
    }

    /// Records `seq`, already delivered, as claimed and being worked on by
    /// the current turn of the session `rec` shows.
    fn mark_delivered(
        &self,
        rec: &AgentRecord,
        store: &ThreadStore,
        seq: u64,
        base: u32,
    ) -> Result<()> {
        store.claim(
            seq,
            &Holder::new(&rec.name, rec.k(), rec.drive_epoch()),
            claim_ttl_ms(),
            ClaimEntries {
                states: vec![TaskState::Claimed, TaskState::Working],
                author: Some(Author::system("dispatch")),
                note: String::new(),
            },
        )?;
        self.update_in(rec, |r| r.task = Some(InFlight { seq, turn: base }))?;
        Ok(())
    }

    /// Keeps the claims of the session `rec` shows while it works: the
    /// task its turn works on (taken again, silently, if its claim was
    /// lost to expiry and nobody else took it), and the tasks it claimed
    /// itself while a turn runs. A session whose task a newer epoch took
    /// over lets go of it.
    pub(crate) async fn renew_claims(
        &self,
        rec: &AgentRecord,
        agents: &cua_spaces::agents::Agents,
        run: &str,
    ) -> Result<()> {
        let store = self.thread_store(&rec.space);
        if !store.exists() {
            return Ok(());
        }
        let holder = Holder::new(&rec.name, rec.k(), rec.drive_epoch());
        let ttl = claim_ttl_ms();
        let current = rec.task.as_ref().map(|t| t.seq);
        if let Some(seq) = current {
            store.renew(&holder, ttl, Some(&[seq]))?;
            if !store.holds(seq, &holder)? {
                match store.claim(seq, &holder, ttl, ClaimEntries::default()) {
                    Ok(_) => {}
                    Err(e) if e.tag() == crate::thread::claims::TASK_CLAIMED => {
                        self.update_in(rec, |r| r.task = None)?;
                    }
                    // Closed or gone: the turn's end tidies up.
                    Err(_) => {}
                }
            }
        }
        let own = holder.to_string();
        let others: Vec<u64> = store
            .claims()?
            .into_iter()
            .filter(|(seq, c)| c.holder == own && Some(*seq) != current)
            .map(|(seq, _)| seq)
            .collect();
        if !others.is_empty()
            && agents
                .status(run)
                .await
                .is_ok_and(|st| st.status == RunStatus::Running)
        {
            store.renew(&holder, ttl, Some(&others))?;
        }
        Ok(())
    }

    /// A durable re-drive of session `k` of `name` at `epoch`: the session
    /// now drives at that epoch, and the claim on the task its turn works
    /// on is taken over at it (before it expires; the old epoch can no
    /// longer renew or reply). The claim is the session's lease: one
    /// mechanism for the inbox and for crash recovery.
    pub fn session_redriven(&self, name: &str, k: u32, epoch: u64) -> Result<()> {
        let rec = self.get(name)?.session(k);
        self.update_in(&rec, |r| r.epoch = r.epoch.max(epoch))?;
        if let Some(t) = &rec.task {
            self.thread_store(&rec.space).claim(
                t.seq,
                &Holder::new(name, rec.k(), epoch),
                claim_ttl_ms(),
                ClaimEntries {
                    states: vec![TaskState::Working],
                    author: Some(Author::system("dispatch")),
                    note: format!("{} re-driven at epoch {epoch}", session_name(&rec)),
                },
            )?;
        }
        Ok(())
    }

    /// `persistent_agent_send` and `agent_message` by name: delivers `text`
    /// as `name`'s next turn (as before) and records it in the thread,
    /// authored by `caller`, as a task the turn's answer replies to.
    pub async fn send_as(
        &self,
        caller: &Caller,
        name: &str,
        text: &str,
    ) -> Result<super::Delivery> {
        let rec = self.get(name)?;
        self.check_right(caller, &rec, Right::Task)?;
        let keys_of = self.keys_of(caller, &rec);
        let base = match self.current_turn(&rec).await {
            Some(Some(t)) => t,
            _ => 0,
        };
        let d = self.send_from(name, text, "persistent").await?;
        let rec = self.get(name)?;
        let store = self.thread_store(&rec.space);
        let mut draft = Draft::new(self.author_for(caller), EntryKind::Message, text);
        draft.mentions = vec![name.to_string()];
        // A task the daemon is still finishing keeps its place; this
        // message is then only recorded.
        let tracked = rec.task.is_none();
        if tracked {
            draft.task = Some(TaskTo {
                to: name.to_string(),
                session: Some(1),
            });
            draft.runs_on_keys_of = keys_of;
        }
        if let Err(e) = check_text(text) {
            // Delivered, but not recorded: the thread never holds a secret.
            tracing::warn!(agent = name, "message not recorded in the thread: {e}");
            return Ok(d);
        }
        let e = store.append(draft)?;
        if tracked {
            self.mark_delivered(&rec, &store, e.seq, if d.started { 0 } else { base })?;
        }
        Ok(d)
    }

    /// When the turn after the in-flight task of the session `rec` shows
    /// ended: posts the turn's answer as its reply (unless it replied
    /// itself) and closes the task. Returns the entry to anchor the turn's
    /// notification to.
    pub(crate) async fn finish_task(
        &self,
        rec: &AgentRecord,
        inflight: &InFlight,
        answer: Option<String>,
        error: Option<String>,
    ) -> Result<Option<u64>> {
        let store = self.thread_store(&rec.space);
        let fold = store.fold()?;
        let mut anchor = None;
        if let Some(t) = fold.tasks.get(&inflight.seq)
            && !t.state.is_closed()
        {
            let mut drafts = vec![];
            if !t.replied {
                let failure = error.as_deref().map(|e| failure_for(rec, e));
                let text = match (&failure, &answer) {
                    // Plain words; the complete error rides along as `failure`.
                    (Some(f), _) => ag::failure::plain(f, &rec.harness),
                    (None, Some(a)) if !a.trim().is_empty() => a.trim().to_string(),
                    _ => "Finished.".to_string(),
                };
                let text = if check_text(&text).is_ok() {
                    text
                } else {
                    "Finished. (The answer looked like it held a secret, so it was not posted.)"
                        .into()
                };
                let mut reply = Draft::new(Author::agent(&rec.name), EntryKind::Message, text);
                reply.reply_to = Some(t.seq);
                reply.failure = failure;
                drafts.push(reply);
            }
            drafts.push(Draft::update(
                Author::system("dispatch"),
                Update {
                    task: t.seq,
                    state: if error.is_some() {
                        TaskState::Failed
                    } else {
                        TaskState::Done
                    },
                    session: Some(rec.k()),
                    lease_until_ms: None,
                    note: String::new(),
                },
            ));
            let written = store.release(t.seq, drafts)?;
            anchor = written.first().map(|e| e.seq);
        }
        self.update_in(rec, |r| r.task = None)?;
        Ok(anchor)
    }

    /// Puts the in-flight task of the session `rec` shows back in the
    /// queue (the member paused).
    pub(crate) fn requeue(&self, rec: &AgentRecord, why: &str) -> Result<()> {
        if let Some(t) = &rec.task {
            self.thread_store(&rec.space).release(
                t.seq,
                vec![Draft::update(
                    Author::system("dispatch"),
                    Update {
                        task: t.seq,
                        state: TaskState::Queued,
                        session: None,
                        lease_until_ms: None,
                        note: why.into(),
                    },
                )],
            )?;
            self.update_in(rec, |r| r.task = None)?;
        }
        Ok(())
    }

    /// Closes `name`'s open tasks (the member was removed).
    pub(crate) fn fail_open_tasks(&self, rec: &AgentRecord, why: &str) -> Result<()> {
        let store = self.thread_store(&rec.space);
        if !store.exists() {
            return Ok(());
        }
        for t in store.fold()?.open_tasks(Some(&rec.name)) {
            store.release(
                t.seq,
                vec![Draft::update(
                    Author::system("dispatch"),
                    Update {
                        task: t.seq,
                        state: TaskState::Failed,
                        session: None,
                        lease_until_ms: None,
                        note: why.into(),
                    },
                )],
            )?;
        }
        Ok(())
    }

    /// A schedule firing (the supervisor fires due routines with it): a
    /// task for the member, authored `system:schedule:<id>`, delivered at
    /// once to its lowest free session. A member with no free session
    /// skips the slot (no task is left behind).
    pub async fn fire_schedule(
        &self,
        routine: &cua_spaces::routines::Routine,
    ) -> Result<std::result::Result<super::Delivery, String>> {
        let rec = self.get(&routine.bot_id)?;
        let busy = || {
            format!(
                "{} is in the middle of a turn; try again when it ends",
                rec.name
            )
        };
        let mut target = None;
        for s in rec.session_views() {
            if s.task.is_some() {
                continue;
            }
            if let Some(None) = self.current_turn(&s).await {
                continue;
            }
            target = Some(s);
            break;
        }
        let Some(target) = target else {
            return Ok(Err(busy()));
        };
        let store = self.thread_store(&rec.space);
        let mut d = Draft::new(
            Author::system(&format!("schedule:{}", routine.id)),
            EntryKind::Message,
            routine.turn_text(),
        );
        d.mentions = vec![rec.name.clone()];
        d.task = Some(TaskTo {
            to: rec.name.clone(),
            session: None,
        });
        let e = store.append(d)?;
        let task = store
            .fold()?
            .tasks
            .get(&e.seq)
            .cloned()
            .ok_or_else(|| Error::invalid("the task vanished"))?;
        match self.deliver(&target, &store, &task, "routine").await? {
            Delivered::Yes(d) => Ok(Ok(d)),
            Delivered::Busy | Delivered::Taken => {
                store.release(
                    e.seq,
                    vec![Draft::update(
                        Author::system("dispatch"),
                        Update {
                            task: e.seq,
                            state: TaskState::Failed,
                            session: None,
                            lease_until_ms: None,
                            note: "skipped: the agent was busy".into(),
                        },
                    )],
                )?;
                Ok(Err(busy()))
            }
        }
    }

    // --- the mirror -----------------------------------------------------

    /// Ships every thread to the Volume (with backoff per thread while the
    /// Volume fails), and rebuilds members' threads that have no local log.
    pub(crate) async fn tick_mirror(&self, report: &mut super::TickReport) {
        let records = self.list().unwrap_or_default();
        let mut spaces: Vec<String> = records.iter().map(|r| r.space.clone()).collect();
        spaces.sort();
        spaces.dedup();
        for s in &spaces {
            self.rebuild_once(&self.thread_store(s)).await;
        }
        let owner = self.owner();
        for store in ThreadStore::list(self.spaces.home_dir()) {
            let key = store.space().to_string();
            // Not shipped to the Volume: see [`HOST_DESKTOP`].
            if key == HOST_DESKTOP {
                continue;
            }
            let now = cua_volume::now_ms();
            if self
                .live
                .lock()
                .await
                .mirror_backoff
                .get(&key)
                .is_some_and(|(_, next)| *next > now)
            {
                continue;
            }
            let members: Vec<Value> = records
                .iter()
                .filter(|r| r.space == key)
                .map(|r| member_row(r, &owner))
                .collect();
            let session = self.drive.session(cua_volume::Context::user());
            let r = tokio::time::timeout(
                super::AGENT_TICK_BUDGET,
                crate::thread::mirror::ship(&store, &session, Some(json!({"members": members}))),
            )
            .await;
            let mut live = self.live.lock().await;
            match r {
                Ok(Ok(_)) => {
                    live.mirror_backoff.remove(&key);
                }
                Ok(Err(e)) => {
                    let fails = live.mirror_backoff.get(&key).map_or(0, |(f, _)| *f) + 1;
                    let wait = (5_000u64 << fails.min(6)).min(300_000);
                    live.mirror_backoff.insert(key.clone(), (fails, now + wait));
                    report.errors.push(format!("thread mirror {key}: {e}"));
                }
                Err(_) => report
                    .errors
                    .push(format!("thread mirror {key}: timed out")),
            }
        }
    }
}

/// Sets `identity.collides` over a Space's member rows.
pub fn with_row_collisions(rows: &mut [Value]) {
    let mut ids: Vec<super::identity::Identity> = rows
        .iter()
        .filter_map(|r| serde_json::from_value(r["identity"].clone()).ok())
        .collect();
    if ids.len() != rows.len() {
        return;
    }
    super::identity::with_collisions(&mut ids);
    for (row, id) in rows.iter_mut().zip(ids) {
        row["identity"]["collides"] = json!(id.collides.unwrap_or(false));
    }
}

/// A member as the tools and the Volume snapshot show it (no env, no
/// secrets).
pub fn member_row(r: &AgentRecord, owner: &Author) -> Value {
    let mut identity = super::identity::identity_of(r, owner);
    identity.collides.get_or_insert(false);
    json!({
        "identity": identity,
        "name": r.name, "handle": r.handle(), "harness": r.harness, "space": r.space, "model": r.model,
        "sessions": r.sessions, "policy": r.policy,
        "added_by": r.added_by.clone().unwrap_or_else(|| owner.clone()),
        "member_since_ms": if r.member_since_ms > 0 { r.member_since_ms } else { r.created_ms },
        "paused": r.paused,
        "rights": Persistent::rights_of(r),
        "held_by": r.held_by,
        "setup": r.setup, "setup_note": r.setup_note,
        "durable_space": r.durable_space,
    })
}

/// One Space's thread, as one caller sees it.
pub struct Thread {
    p: Persistent,
    store: ThreadStore,
    caller: Caller,
    /// An agent caller's session (its bridge knows which run it is).
    session: Option<u32>,
}

impl Thread {
    /// The thread as session `k` of the calling agent.
    pub fn as_session(mut self, k: u32) -> Thread {
        self.session = Some(k.max(1));
        self
    }

    pub fn store(&self) -> &ThreadStore {
        &self.store
    }

    fn me(&self) -> Author {
        self.p.author_for(&self.caller)
    }

    /// The caller as the author of a new entry: an agent member's carries
    /// its identity as of now.
    fn author(&self) -> Author {
        let mut a = self.me();
        if let Some((name, _)) = self.caller.agent_member()
            && let Ok(rec) = self.p.get(name)
        {
            let mut id = super::identity::identity_of(&rec, &self.p.owner());
            id.last_seen_ms = None;
            a.name = id.display_name.clone();
            a.identity = Some(Box::new(id));
        }
        a
    }

    /// The names of the Space's agent members.
    fn member_names(&self) -> Result<Vec<String>> {
        Ok(self
            .p
            .members_of(self.store.space())?
            .into_iter()
            .map(|r| r.handle().to_string())
            .collect())
    }

    /// The key of this Space's member that answers to `@handle`.
    fn member_key(&self, handle: &str) -> Option<String> {
        self.p
            .members_of(self.store.space())
            .ok()?
            .into_iter()
            .find(|r| r.handle() == handle)
            .map(|r| r.name)
    }

    /// Posts as the caller. An @mention of an agent member (not the author
    /// itself) makes the entry a task for the first one mentioned.
    pub async fn post(&self, args: PostArgs) -> Result<ThreadEntry> {
        policy::may_write(self.p.role_of(&self.caller))?;
        check_text(&args.text)?;
        // An answer to an approval or join request may carry no words.
        if args.text.trim().is_empty() && args.attachments.is_empty() && args.answer.is_none() {
            return Err(Error::invalid("nothing to post: give text or attachments"));
        }
        if let Caller::Member { handle, .. } = &self.caller {
            self.p.post_allowed(handle, &self.store).await?;
        }
        let author = self.author();
        let members = self.member_names()?;
        // `@claude` of this Space is the member's key (`claude-2` in a second Space).
        let mentions: Vec<String> = parse_mentions(&args.text)
            .into_iter()
            .filter(|m| members.contains(m))
            .filter_map(|m| self.member_key(&m))
            .collect();
        let fold = self.store.fold()?;
        let mut kind = EntryKind::Message;
        let mut draft_approval = None;
        let mut draft_answer = None;
        if let Some((action, details)) = args.approval {
            refuse_secrets("approval", &format!("{action}\n{details}"))?;
            kind = EntryKind::Approval;
            draft_approval = Some(Approval {
                action,
                details,
                state: "open".into(),
            });
        }
        if let Some((seq, approve, note)) = args.answer {
            if kind == EntryKind::Approval {
                return Err(Error::invalid("give approval or answer, not both"));
            }
            refuse_secrets("note", &note)?;
            match fold.approvals.get(&seq) {
                None => return Err(Error::NotFound(format!("approval #{seq} in this thread"))),
                Some(true) => return Err(Error::invalid(format!("approval #{seq} was answered"))),
                Some(false) => {}
            }
            self.may_answer(seq)?;
            kind = EntryKind::Answer;
            draft_answer = Some(Answer {
                approval: seq,
                approve,
                note,
            });
        }
        if let Some(r) = args.reply_to
            && r > self.store.last_seq()?
        {
            return Err(Error::NotFound(format!("entry #{r} in this thread")));
        }
        let view = args
            .view
            .map(crate::thread::ViewContext::checked)
            .transpose()?;
        let id = new_entry_id();
        let attachments = self.attachments(&id, &args.attachments).await?;
        let task_to = mentions
            .iter()
            .find(|m| author.agent_name() != Some(m.as_str()))
            .cloned();
        // A mention of an agent the caller can't task is posted, but gives
        // no task (a note says why).
        let mut refused = None;
        let mut keys_of = None;
        let task_to = match task_to.map(|to| (self.p.get(&to), to)) {
            Some((Ok(rec), to)) => {
                if self.p.allows(&self.caller, &rec, Right::Task) {
                    keys_of = self.p.keys_of(&self.caller, &rec);
                    Some(to)
                } else {
                    refused = Some((to, Persistent::rights_of(&rec).task));
                    None
                }
            }
            Some((Err(_), to)) => Some(to),
            None => None,
        };
        let who = author.name.clone();
        let answered_join = draft_answer
            .as_ref()
            .filter(|a| fold.join_requests.contains_key(&a.approval))
            .map(|a| (a.approval, a.approve));
        if let Some(k) = args.to_session {
            let to = task_to.as_deref().ok_or_else(|| {
                Error::invalid("to_session needs an @mention of an agent member of this Space")
            })?;
            let n = self.p.get(to)?.sessions.max(1);
            if k == 0 || k > n {
                return Err(Error::invalid(format!(
                    "to_session {k}: {to} has {n} session{}",
                    if n == 1 { "" } else { "s" }
                )));
            }
        }
        let mut d = Draft::new(author, kind, args.text);
        d.id = Some(id);
        d.mentions = mentions;
        d.reply_to = args.reply_to.or(draft_answer.as_ref().map(|a| a.approval));
        d.attachments = attachments;
        d.approval = draft_approval;
        d.answer = draft_answer;
        d.task = task_to.clone().map(|to| TaskTo {
            to,
            session: args.to_session,
        });
        d.runs_on_keys_of = keys_of;
        d.view = view;
        let e = self.store.append(d)?;
        if let Some((seq, approve)) = answered_join {
            self.p
                .answer_join_request(&self.caller, &self.store, seq, approve)?;
        }
        if let Some((to, holders)) = refused {
            self.store.append(Draft::note(
                "rights",
                None,
                format!(
                    "{who} can't give {to} tasks, so no task was made. Only {} can.",
                    holders.describe()
                ),
            ))?;
        }
        if let Some(to) = task_to
            && let Ok(rec) = self.p.get(&to)
            && rec.paused
        {
            self.store.append(Draft::note(
                "dispatch",
                Some(e.seq),
                format!("{to} is paused; task #{} waits until it is resumed.", e.seq),
            ))?;
        }
        Ok(e)
    }

    /// Resolves attachments: Volume paths the caller can read are
    /// referenced; a local file (this machine's own user only) is copied to
    /// `spaces/<space>/thread/files/<entry id>/<name>`.
    async fn attachments(&self, entry_id: &str, paths: &[String]) -> Result<Vec<Attachment>> {
        if paths.len() > MAX_ATTACHMENTS {
            return Err(Error::invalid(format!(
                "{} attachments; at most {MAX_ATTACHMENTS}",
                paths.len()
            )));
        }
        let user = self.p.drive.session(cua_volume::Context::user());
        let folder = self.store.volume_folder();
        let mut out = vec![];
        for p in paths {
            let name = Path::new(p.trim_end_matches('/'))
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .filter(|n| !n.is_empty())
                .ok_or_else(|| Error::invalid(format!("attachment {p:?} has no file name")))?;
            refuse_secrets("attachment name", &name)?;
            if is_local_path(p) {
                if self.caller != Caller::Owner {
                    return Err(denied(
                        "only this machine's own user attaches local files; attach a Cua Volume path",
                    ));
                }
                let local = expand_home(p);
                let meta = std::fs::metadata(&local)
                    .map_err(|e| Error::invalid(format!("attachment {p}: {e}")))?;
                if !meta.is_file() {
                    return Err(Error::invalid(format!("attachment {p} is not a file")));
                }
                if meta.len() > MAX_LOCAL_ATTACHMENT {
                    return Err(Error::invalid(format!(
                        "attachment {p} is {} bytes; at most {MAX_LOCAL_ATTACHMENT}",
                        meta.len()
                    )));
                }
                let bytes = std::fs::read(&local)?;
                if let Some(f) = cua_volume::scan::scan(&bytes).first() {
                    return Err(Error::invalid(format!(
                        "looks like a secret; use the Keyvault ({} on line {} of {name})",
                        f.kind, f.line
                    )));
                }
                let key = format!("{folder}thread/files/{entry_id}/{name}");
                let m = user
                    .write(&key, bytes, cua_volume::Condition::None)
                    .await
                    .drive()?;
                out.push(Attachment {
                    path: m.key,
                    mime: mime_of(&name).into(),
                    name,
                    size: m.size,
                });
                continue;
            }
            let key = cua_volume::path::normalize(p).drive()?;
            let meta = match &self.caller {
                Caller::Agent { name: agent, space } => self
                    .p
                    .drive
                    .session(cua_volume::Context::agent(agent, Some(space)))
                    .open(&key, None)
                    .await
                    .drive()?,
                Caller::Relay { .. } | Caller::Member { .. } if !key.starts_with(&folder) => {
                    return Err(denied(format!(
                        "attach files from this Space's folder ({folder})"
                    )));
                }
                _ => user.open(&key, None).await.drive()?,
            };
            out.push(Attachment {
                path: meta.key,
                mime: mime_of(&name).into(),
                name,
                size: meta.size,
            });
        }
        Ok(out)
    }

    /// A page of the thread: entries after `after` (reading on), or with
    /// no `after` the newest `limit` before `before` (default: the end),
    /// oldest first either way. With the caller (`me`), its read position,
    /// its unread count and the open tasks; `mark_read` moves the caller's
    /// cursor to the last entry returned.
    ///
    /// `with` (an agent member's handle) narrows the page to the caller's DM
    /// with it ([`crate::thread::dm_entries`], CUA-2266), with its own read
    /// cursor.
    pub fn read(
        &self,
        after: Option<u64>,
        before: Option<u64>,
        limit: usize,
        mark_read: bool,
        with: Option<&str>,
    ) -> Result<Value> {
        let me = self.me();
        let limit = limit.clamp(1, 500);
        let last = self.store.last_seq()?;
        if let Some(with) = with {
            return self.read_dm(&me, with, after, before, limit, mark_read, last);
        }
        let (entries, has_more_before) = match after {
            Some(after) => {
                let e = self.store.read(after, limit)?;
                let more = after > 0
                    && self
                        .store
                        .read(0, 1)?
                        .first()
                        .is_some_and(|x| x.seq <= after);
                (e, more)
            }
            None => self.store.read_before(before, limit)?,
        };
        let cursor = entries.last().map_or(after.unwrap_or(last), |e| e.seq);
        if mark_read && cursor > 0 {
            self.store.mark_read(&me.id, cursor)?;
        }
        let read_seq = self.store.cursors()?.get(&me.id).map_or(0, |c| c.read_seq);
        // A join request says whether this caller may answer it.
        let fold = self.store.fold()?;
        let answers_joins = matches!(
            self.p.role_of(&self.caller),
            policy::Role::Owner | policy::Role::Admin
        );
        let entries: Vec<Value> = entries
            .into_iter()
            .map(|e| {
                let open =
                    e.join_request.is_some() && fold.join_requests.get(&e.seq) == Some(&None);
                let mut v = serde_json::to_value(&e).unwrap_or(Value::Null);
                if e.join_request.is_some() {
                    v["you_can_answer"] = json!(open && answers_joins);
                }
                v
            })
            .collect();
        Ok(json!({
            "space": self.store.space(),
            "me": me,
            // What the caller is here (`owner`, `admin`, `member`, `guest`,
            // `agent`): a guest reads only.
            "role": self.p.role_of(&self.caller).as_str(),
            "entries": entries,
            "cursor": cursor,
            "caught_up": cursor >= last,
            "has_more_before": has_more_before,
            "read_seq": read_seq,
            "unread": self.store.unread(&me.id)?,
            "tasks": self.store.with_claims(fold.open_tasks(None))?,
        }))
    }

    /// The caller's DM with agent `with`: [`Thread::read`]'s page over the
    /// DM's entries, and its own cursor (the thread's stands in until the
    /// DM has one).
    #[allow(clippy::too_many_arguments)]
    fn read_dm(
        &self,
        me: &Author,
        with: &str,
        after: Option<u64>,
        before: Option<u64>,
        limit: usize,
        mark_read: bool,
        last: u64,
    ) -> Result<Value> {
        let all = crate::thread::dm_entries(&self.store.read(0, usize::MAX)?, &me.id, with);
        let (entries, has_more_before): (Vec<_>, bool) = match after {
            Some(after) => (
                all.iter()
                    .filter(|e| e.seq > after)
                    .take(limit)
                    .cloned()
                    .collect(),
                all.first().is_some_and(|e| e.seq <= after),
            ),
            None => {
                let older: Vec<_> = all
                    .iter()
                    .filter(|e| before.is_none_or(|b| e.seq < b))
                    .cloned()
                    .collect();
                let skip = older.len().saturating_sub(limit);
                (older[skip..].to_vec(), skip > 0)
            }
        };
        // Following on: the next read starts after what this page covered,
        // or the thread's end when the DM had nothing new.
        let cursor = entries.last().map_or(after.unwrap_or(last), |e| e.seq);
        let key = crate::thread::dm_cursor_key(&me.id, with);
        if mark_read && cursor > 0 {
            self.store.mark_read(&key, cursor)?;
        }
        let cursors = self.store.cursors()?;
        let read_seq = cursors
            .get(&key)
            .or_else(|| cursors.get(&me.id))
            .map_or(0, |c| c.read_seq);
        let unread = all
            .iter()
            .filter(|e| e.seq > read_seq && e.author.id != me.id)
            .count();
        Ok(json!({
            "space": self.store.space(),
            "me": me,
            "role": self.p.role_of(&self.caller).as_str(),
            "with": with,
            "entries": entries,
            "cursor": cursor,
            "caught_up": cursor >= last,
            "has_more_before": has_more_before,
            "read_seq": read_seq,
            "unread": unread,
        }))
    }

    /// The caller's DM with agent `with`, summed up for the Agents tab.
    pub fn dm_summary(&self, with: &str, may_approve: bool) -> Result<crate::thread::DmSummary> {
        let me = self.me();
        let cursors = self.store.cursors()?;
        let key = crate::thread::dm_cursor_key(&me.id, with);
        let read = cursors
            .get(&key)
            .or_else(|| cursors.get(&me.id))
            .map_or(0, |c| c.read_seq);
        Ok(crate::thread::dm_summary(
            &self.store.read(0, usize::MAX)?,
            &me.id,
            &me.name,
            with,
            read,
            &self.store.fold()?.approvals,
            may_approve,
        ))
    }

    /// Open tasks of `member` (an agent caller: its own), oldest first.
    pub fn inbox(&self, member: Option<&str>) -> Result<Vec<TaskInfo>> {
        let member = match self.caller.agent_member() {
            Some((name, _)) => Some(name),
            None => member,
        };
        self.store
            .with_claims(self.store.fold()?.open_tasks(member))
    }

    fn agent_of(&self, member: Option<&str>, task: &TaskInfo) -> Result<String> {
        match self.caller.agent_member() {
            Some((name, _)) => {
                if task.to != name {
                    return Err(denied(format!(
                        "task #{} is for {}, not {name}",
                        task.seq, task.to
                    )));
                }
                Ok(name.to_string())
            }
            None => {
                let m = member.unwrap_or(&task.to);
                if m != task.to {
                    return Err(Error::invalid(format!(
                        "task #{} is for {}, not {m}",
                        task.seq, task.to
                    )));
                }
                Ok(m.to_string())
            }
        }
    }

    /// A person claiming or replying for an agent needs control of it (or
    /// to have asked for the task); the agent itself always may.
    fn may_steer(&self, t: &TaskInfo) -> Result<()> {
        policy::may_write(self.p.role_of(&self.caller))?;
        if self.caller.agent_member().is_some() {
            return Ok(());
        }
        let Ok(rec) = self.p.get(&t.to) else {
            return Ok(());
        };
        if t.author.id == self.me().id {
            return Ok(());
        }
        self.p.check_control(&self.caller, &rec)
    }

    /// Answering approval `seq`: the `approve` right of the agent that
    /// asked (the defaults when a person asked or the agent is gone).
    fn may_answer(&self, seq: u64) -> Result<()> {
        if self.store.fold()?.join_requests.contains_key(&seq) {
            return match self.p.role_of(&self.caller) {
                policy::Role::Owner | policy::Role::Admin => Ok(()),
                _ => Err(denied(
                    "only the owner and admins let an agent join this Space",
                )),
            };
        }
        let asker = self
            .store
            .get(seq)?
            .and_then(|e| e.author.agent_name().map(str::to_string));
        let rec = asker.and_then(|n| self.p.get(&n).ok());
        let rights = rec.as_ref().map(Persistent::rights_of).unwrap_or_default();
        let allowed = match &rec {
            Some(r) => self.p.allows(&self.caller, r, Right::Approve),
            None => rights.allows(Right::Approve, self.p.role_of(&self.caller), &self.me().id),
        };
        if allowed {
            return Ok(());
        }
        let agent = rec.map_or_else(|| "this agent".to_string(), |r| r.name);
        Err(policy::refuse(Right::Approve, &agent, &rights.approve))
    }

    fn task(&self, seq: u64) -> Result<TaskInfo> {
        self.store
            .fold()?
            .tasks
            .get(&seq)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("task #{seq} in this thread")))
    }

    /// Claims task `seq` for one session of its member, with a lease (see
    /// [`crate::thread::claims`]): an agent's own session (from its
    /// bridge), else `session`, else the member's lowest session with no
    /// task. A task another session holds is refused with `task_claimed`
    /// and the holder. The result is the `claimed` entry plus `claim`
    /// (`holder`, `session`, `sessions`, `lease_until_ms`).
    pub fn claim(&self, seq: u64, member: Option<&str>, session: Option<u32>) -> Result<Value> {
        let t = self.task(seq)?;
        let name = self.agent_of(member, &t)?;
        self.may_steer(&t)?;
        let rec = self.p.get(&name)?;
        let n = rec.sessions.max(1);
        let k = match (self.session, session) {
            (Some(k), _) => k,
            (None, Some(k)) if (1..=n).contains(&k) => k,
            (None, Some(k)) => {
                return Err(Error::invalid(format!(
                    "session {k}: {name} has {n} session{}",
                    if n == 1 { "" } else { "s" }
                )));
            }
            (None, None) => rec
                .session_views()
                .iter()
                .find(|s| s.task.is_none())
                .map_or(1, |s| s.k()),
        };
        let view = rec.session(k);
        let holder = Holder::new(&name, k, view.drive_epoch());
        let (claim, mut written) = self.store.claim(
            seq,
            &holder,
            claim_ttl_ms(),
            ClaimEntries {
                states: vec![TaskState::Claimed],
                author: Some(self.me()),
                note: String::new(),
            },
        )?;
        let mut out = serde_json::to_value(written.remove(0))?;
        out["claim"] = json!({
            "holder": claim.holder, "session": k, "sessions": n,
            "lease_until_ms": claim.expires_ms,
        });
        Ok(out)
    }

    /// Replies under task `seq` and sets its state (`done` by default).
    pub async fn reply(
        &self,
        seq: u64,
        text: &str,
        attachments: &[String],
        state: Option<&str>,
    ) -> Result<(ThreadEntry, ThreadEntry)> {
        check_text(text)?;
        if let Caller::Member { handle, .. } = &self.caller {
            self.p.post_allowed(handle, &self.store).await?;
        }
        let state = match state {
            Some(s) => TaskState::parse(s)?,
            None => TaskState::Done,
        };
        let t = self.task(seq)?;
        let name = self.agent_of(None, &t)?;
        self.may_steer(&t)?;
        if t.state.is_closed() {
            return Err(Error::invalid(format!(
                "task #{seq} is {}",
                t.state.as_str()
            )));
        }
        // An agent session answers only what no other session holds.
        let mine = match self.session {
            Some(k) => {
                let epoch = self.p.get(&name)?.session(k).drive_epoch();
                let me = Holder::new(&name, k, epoch);
                let now = cua_volume::now_ms();
                if let Some(c) = self.store.claims()?.get(&seq).filter(|c| c.live(now))
                    && c.holder != me.to_string()
                {
                    return Err(crate::thread::claims::claimed(seq, &c.holder, c.expires_ms));
                }
                Some(me)
            }
            None => None,
        };
        let id = new_entry_id();
        let files = self.attachments(&id, attachments).await?;
        // A reply is authored by the task's agent (a person replying for
        // it through this tool is recorded as themselves).
        let mut reply = Draft::new(self.author(), EntryKind::Message, text);
        reply.id = Some(id);
        reply.reply_to = Some(seq);
        reply.mentions = parse_mentions(text);
        reply.attachments = files;
        let update = Draft::update(
            self.me(),
            Update {
                task: seq,
                state,
                session: mine.as_ref().map(|h| h.session).or(t.session).or(Some(1)),
                lease_until_ms: (state == TaskState::Working)
                    .then(|| cua_volume::now_ms() + claim_ttl_ms()),
                note: String::new(),
            },
        );
        let mut w = if state.is_closed() {
            self.store.release(seq, vec![reply, update])?
        } else {
            let w = self.store.append_many(vec![reply, update])?;
            // Still working: the agent's session holds it on.
            if let Some(h) = &mine {
                self.store
                    .claim(seq, h, claim_ttl_ms(), ClaimEntries::default())?;
            }
            w
        };
        let update = w.pop().expect("two");
        let reply = w.pop().expect("two");
        Ok((reply, update))
    }

    /// Imports an app's earlier conversation once (this machine's own user
    /// only). Entries already imported (by `import_id`) are skipped.
    pub fn import(&self, source: &str, entries: Vec<ImportEntry>) -> Result<Value> {
        if self.caller != Caller::Owner {
            return Err(denied("only this machine's own user imports into a thread"));
        }
        let source = source.trim();
        if source.is_empty() || source.len() > 64 {
            return Err(Error::invalid("source: 1-64 characters"));
        }
        if entries.len() > MAX_IMPORT {
            return Err(Error::invalid(format!(
                "{} entries; at most {MAX_IMPORT} per call",
                entries.len()
            )));
        }
        let members = self.member_names()?;
        let fold = self.store.fold()?;
        let owner = self.p.owner();
        let mut seen = std::collections::HashSet::new();
        let mut drafts = vec![];
        let mut skipped = 0usize;
        for (i, e) in entries.into_iter().enumerate() {
            if e.import_id.trim().is_empty() {
                return Err(Error::invalid(format!(
                    "entries[{i}]: import_id is required"
                )));
            }
            if fold.imported(source, &e.import_id) || !seen.insert(e.import_id.clone()) {
                skipped += 1;
                continue;
            }
            check_text(&e.text).map_err(|err| Error::invalid(format!("entries[{i}]: {err}")))?;
            let author = match e.author_kind.as_str() {
                "person" => owner.clone(),
                "agent" => {
                    let name = e.agent_name.as_deref().unwrap_or_default();
                    if !members.iter().any(|m| m == name) {
                        return Err(Error::invalid(format!(
                            "entries[{i}]: {name:?} is not an agent member of {}",
                            self.store.space()
                        )));
                    }
                    Author::agent(name)
                }
                other => {
                    return Err(Error::invalid(format!(
                        "entries[{i}]: author_kind {other:?}: use person or agent"
                    )));
                }
            };
            let kind = match e.kind.as_deref().unwrap_or("message") {
                "message" => EntryKind::Message,
                "approval" => EntryKind::Approval,
                "answer" => EntryKind::Answer,
                other => {
                    return Err(Error::invalid(format!(
                        "entries[{i}]: kind {other:?}: use message, approval or answer"
                    )));
                }
            };
            let mut d = Draft::new(author, kind, e.text);
            d.ts_ms = Some(e.ts_ms);
            d.import = Some(Import {
                source: source.to_string(),
                id: e.import_id,
            });
            drafts.push(d);
        }
        let n = drafts.len();
        self.store.append_many(drafts)?;
        let note = if n > 0 {
            Some(self.store.append(Draft::new(
                Author::system("import"),
                EntryKind::System,
                format!("imported {n} messages from {source}"),
            ))?)
        } else {
            None
        };
        Ok(json!({"imported": n, "skipped": skipped, "note": note}))
    }
}

/// The claim TTL in milliseconds.
pub(crate) fn claim_ttl_ms() -> u64 {
    CLAIM_TTL.as_millis() as u64
}

/// What [`Persistent::deliver`] did.
#[derive(Debug)]
pub(crate) enum Delivered {
    Yes(super::Delivery),
    /// The session is mid turn; the task stays queued.
    Busy,
    /// Another session claimed the task first.
    Taken,
}

/// `ada`, or `ada (session 2)`.
fn session_name(rec: &AgentRecord) -> String {
    match rec.k() {
        1 => rec.name.clone(),
        k => format!("{} (session {k})", rec.name),
    }
}

/// The typed failure of `rec`'s turn or start with the error text `e`: its
/// class, the provider to connect, and the complete text (a text that looks
/// like a secret is withheld, as everything posted to the thread is).
pub(crate) fn failure_for(rec: &AgentRecord, e: &str) -> ag::failure::Failure {
    let mut f = ag::failure::classify(&rec.harness, e);
    if check_text(&f.message).is_err() {
        f.message = "(details withheld: they looked like they held a secret)".into();
    }
    f
}

/// A failed member's status line: "Not connected" when it needs a sign-in,
/// else the error's first line, whole.
fn failed_line(harness: &str, e: &str) -> String {
    let f = ag::failure::classify(harness, e);
    match f.class {
        ag::failure::FailureClass::NeedsConnect => "Not connected".into(),
        ag::failure::FailureClass::Error => super::notify::reason(e),
    }
}

/// One session of an agent member, as `agent_members` shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionState {
    /// 1-based.
    pub k: u32,
    /// `working | idle | waiting | paused | asleep | failed`.
    pub state: &'static str,
    /// One short line: what it is doing (the latest tool or plan step),
    /// what it waits for, or why it failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    /// For a failed session: what failed, typed (`needs_connect` and the
    /// provider, or an error) with the complete text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<ag::failure::Failure>,
    /// The task its turn works on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_seq: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// The state of the session `s` shows, from its run's status (`None`:
/// unknown) and the thread.
pub fn session_state(
    s: &AgentRecord,
    run: Option<RunStatus>,
    fold: &crate::thread::Fold,
) -> SessionState {
    let task_seq = s.task.as_ref().map(|t| t.seq);
    let asked = task_seq.and_then(|t| fold.open_approval_under(&s.name, t));
    let failed = matches!(run, Some(RunStatus::Failed | RunStatus::Crashed));
    let state = if s.paused {
        "paused"
    } else if asked.is_some() {
        "waiting"
    } else {
        match (&s.run_id, run) {
            (Some(_), Some(RunStatus::Running)) => "working",
            _ if failed => "failed",
            // Between the turn's end and the supervisor closing its task.
            (Some(_), _) if task_seq.is_some() => "working",
            (Some(_), Some(RunStatus::Idle)) => "idle",
            _ => "asleep",
        }
    };
    let activity = match state {
        "working" => s.activity.clone().or_else(|| {
            task_seq
                .and_then(|seq| fold.tasks.get(&seq))
                .map(|t| format!("task #{}: {}", t.seq, clip_words(&t.text, 80)))
        }),
        "waiting" => asked.map(|a| format!("waiting for an answer to #{a}")),
        "failed" => s.last_error.as_deref().map(|e| failed_line(&s.harness, e)),
        _ => None,
    };
    // An error is never cut: only a working line is clipped.
    let activity = if state == "failed" {
        activity
    } else {
        activity.map(|a| clip_line(&a, 80))
    };
    let failure = (state == "failed")
        .then(|| s.last_error.as_deref().map(|e| failure_for(s, e)))
        .flatten();
    SessionState {
        k: s.k(),
        state,
        activity,
        failure,
        task_seq,
        run_id: s.run_id.clone(),
    }
}

/// The member's state: paused, or the busiest of its sessions.
pub fn member_state(paused: bool, sessions: &[SessionState]) -> &'static str {
    if paused {
        return "paused";
    }
    ["working", "waiting", "failed", "idle"]
        .into_iter()
        .find(|st| sessions.iter().any(|s| s.state == *st))
        .unwrap_or("asleep")
}

/// `s` on one line, at most `n` characters.
pub(crate) fn clip_line(s: &str, n: usize) -> String {
    clip_words(s, n)
}

/// The first `n` characters of `s` on one line.
fn clip_words(s: &str, n: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        return one;
    }
    let mut out: String = one.chars().take(n.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}
