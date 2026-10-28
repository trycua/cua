// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A Space's thread: one append-only log per Space, owned by the daemon
//! that holds the Space's record and mirrored to the Cua Volume.
//!
//! On the owner (the source of truth), under
//! `<cua home>/threads/<space folder>/`:
//!
//! | File | What |
//! |---|---|
//! | `log/<first seq, 12 digits>.jsonl` | Entries, one JSON per line, fsynced. A segment closes at 4 MiB or 10 000 entries; closed segments never change. |
//! | `head.json` | `next_seq`, the open segment and its size, `mirrored_seq` and the mirror's bookkeeping, written atomically |
//! | `cursors.json` | `{"<member id>": {"read_seq", "updated_ms"}}`: each member's read position |
//! | `space.json` | The Space id the folder belongs to |
//! | `thread.lock` | Held (an exclusive file lock) for every write, by any process |
//!
//! Entries are never edited. A task (a message that @mentions an agent
//! member) gets its state from later `task_update` entries; [`Fold`] folds
//! them. Authors are stamped by the daemon from the authenticated caller
//! ([`Author`]), never taken from arguments. Text and attachment names pass
//! the Cua Volume's secret scanner before anything is written.

pub mod claims;
pub mod mirror;
pub mod policy;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use cua_spaces::{Error, Result};

/// The daemon feature (`GetInfoResponse.features`) of a daemon that serves
/// Space threads and agent members.
pub const FEATURE: &str = "thread.v1";
/// The entry format.
pub const VERSION: u32 = 1;
/// Longest entry text.
pub const MAX_TEXT: usize = 32 * 1024;
/// Most attachments per entry.
pub const MAX_ATTACHMENTS: usize = 16;
/// A segment closes at this size...
pub const SEGMENT_BYTES: u64 = 4 * 1024 * 1024;
/// ...or this many entries.
pub const SEGMENT_ENTRIES: u64 = 10_000;
/// The longest `thread_read` long poll.
pub const MAX_FOLLOW: Duration = Duration::from_secs(25);
/// The folder under the cua home.
pub const DIR: &str = "threads";

// ------------------------------------------------------------------ types

/// Who wrote an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorKind {
    Person,
    Agent,
    System,
}

/// An entry's author, stamped by the daemon.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Author {
    pub kind: AuthorKind,
    /// A person: the relay account id (`owner` on a machine that is not
    /// signed in); an agent: `agent:<name>`; the daemon: `system:<what>`.
    pub id: String,
    /// Display name.
    #[serde(default)]
    pub name: String,
    /// An agent's identity when it wrote the entry (its mark, owner and
    /// device stay with its posts after it leaves).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Box<crate::persistent::identity::Identity>>,
}

impl Author {
    /// This machine's user when no account identity is known.
    pub fn owner() -> Author {
        Author {
            kind: AuthorKind::Person,
            id: "owner".into(),
            name: "Owner".into(),
            identity: None,
        }
    }

    /// The agent member `name`.
    pub fn agent(name: &str) -> Author {
        Author {
            kind: AuthorKind::Agent,
            id: format!("agent:{name}"),
            name: name.into(),
            identity: None,
        }
    }

    /// The daemon itself (`system:<what>`).
    pub fn system(what: &str) -> Author {
        Author {
            kind: AuthorKind::System,
            id: format!("system:{what}"),
            name: "Cua".into(),
            identity: None,
        }
    }

    /// The agent member's name, for an agent author.
    pub fn agent_name(&self) -> Option<&str> {
        match self.kind {
            AuthorKind::Agent => self.id.strip_prefix("agent:"),
            _ => None,
        }
    }
}

/// What an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Message,
    TaskUpdate,
    Approval,
    Answer,
    System,
    /// An agent asking to join (answered like an approval).
    JoinRequest,
    /// A kind written by a newer daemon.
    #[serde(other)]
    Other,
}

/// A task's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Queued,
    Claimed,
    Working,
    Done,
    Failed,
}

impl TaskState {
    /// Done or failed.
    pub fn is_closed(self) -> bool {
        matches!(self, TaskState::Done | TaskState::Failed)
    }

    pub fn parse(s: &str) -> Result<TaskState> {
        Ok(match s.trim() {
            "done" => TaskState::Done,
            "failed" => TaskState::Failed,
            "working" => TaskState::Working,
            other => {
                return Err(Error::invalid(format!(
                    "state {other:?}: use done, failed or working"
                )));
            }
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Queued => "queued",
            TaskState::Claimed => "claimed",
            TaskState::Working => "working",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
        }
    }
}

/// A message addressed to an agent member.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskTo {
    pub to: String,
    #[serde(default)]
    pub session: Option<u32>,
}

/// What the person was looking at in the Space when they posted: its
/// desktop, or one of its windows (`thread_post`'s `view`). Stored with the
/// entry, so an agent reading the thread or its inbox knows what a message
/// is about ("this tab" is the Firefox window titled "Pull requests"), and
/// clients show it as one quiet line ([`ViewContext::line`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewContext {
    /// `desktop` or `window`.
    pub kind: String,
    /// The window's app ("Firefox").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// The window's title ("Pull requests").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The window's id in the Space (what `computer_window_*` and the
    /// Space's window stream take).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
}

/// The longest app name a view keeps.
pub const VIEW_APP_MAX: usize = 128;
/// The longest window title a view keeps (longer ones are cut).
pub const VIEW_TITLE_MAX: usize = 256;
/// The longest window id a view keeps.
pub const VIEW_WINDOW_ID_MAX: usize = 128;

impl ViewContext {
    /// The kinds a view can be.
    pub const KINDS: [&'static str; 2] = ["desktop", "window"];

    /// The view as stored: kind checked, blanks dropped, the title cut to
    /// [`VIEW_TITLE_MAX`] characters. A desktop carries no window fields; a
    /// window needs its id, app or title. A title that looks like a secret
    /// is refused, as a message would be.
    pub fn checked(self) -> Result<ViewContext> {
        let kind = self.kind.trim().to_ascii_lowercase();
        if !Self::KINDS.contains(&kind.as_str()) {
            return Err(Error::invalid(format!(
                "view.kind: `{}` is not desktop or window",
                self.kind
            )));
        }
        let keep = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let app = keep(self.app);
        let title = keep(self.title).map(|t| clip_chars(&t, VIEW_TITLE_MAX));
        let window_id = keep(self.window_id);
        if app
            .as_ref()
            .is_some_and(|a| a.chars().count() > VIEW_APP_MAX)
        {
            return Err(Error::invalid(format!(
                "view.app: at most {VIEW_APP_MAX} characters"
            )));
        }
        if window_id
            .as_ref()
            .is_some_and(|w| w.chars().count() > VIEW_WINDOW_ID_MAX)
        {
            return Err(Error::invalid(format!(
                "view.window_id: at most {VIEW_WINDOW_ID_MAX} characters"
            )));
        }
        if let Some(t) = &title {
            refuse_secrets("view title", t)?;
        }
        if kind == "desktop" {
            return Ok(ViewContext {
                kind,
                ..Default::default()
            });
        }
        if app.is_none() && title.is_none() && window_id.is_none() {
            return Err(Error::invalid(
                "view: a window needs its window_id, app or title",
            ));
        }
        Ok(ViewContext {
            kind,
            app,
            title,
            window_id,
        })
    }

    /// The quiet line clients show under the message: "Sent from Firefox",
    /// the title when the app is not known ("Sent from Pull requests"), or
    /// "Sent from the desktop".
    pub fn line(&self) -> String {
        match (self.kind.as_str(), &self.app, &self.title) {
            ("window", Some(app), _) => format!("Sent from {app}"),
            ("window", None, Some(title)) => format!("Sent from {title}"),
            ("window", None, None) => "Sent from a window".into(),
            _ => "Sent from the desktop".into(),
        }
    }
}

/// The first `n` characters of `s`.
fn clip_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// The proto twin (`cua.env.v1.ThreadView`): tests hold the JSON names to it.
#[cfg(test)]
impl From<&ViewContext> for cua_proto::env::v1::ThreadView {
    fn from(v: &ViewContext) -> Self {
        cua_proto::env::v1::ThreadView {
            kind: v.kind.clone(),
            app: v.app.clone().unwrap_or_default(),
            title: v.title.clone().unwrap_or_default(),
            window_id: v.window_id.clone().unwrap_or_default(),
        }
    }
}

#[cfg(test)]
impl From<cua_proto::env::v1::ThreadView> for ViewContext {
    fn from(v: cua_proto::env::v1::ThreadView) -> Self {
        let some = |s: String| (!s.is_empty()).then_some(s);
        ViewContext {
            kind: v.kind,
            app: some(v.app),
            title: some(v.title),
            window_id: some(v.window_id),
        }
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;

    fn window(app: &str, title: &str, id: &str) -> ViewContext {
        ViewContext {
            kind: "window".into(),
            app: Some(app.into()),
            title: Some(title.into()),
            window_id: Some(id.into()),
        }
    }

    #[test]
    fn a_window_view_keeps_its_app_title_and_id_and_says_where_it_came_from() {
        let v = window(" Firefox ", "Pull requests", "w42")
            .checked()
            .unwrap();
        assert_eq!(v.app.as_deref(), Some("Firefox"));
        assert_eq!(v.line(), "Sent from Firefox");
        let untitled_app = ViewContext {
            kind: "WINDOW".into(),
            title: Some("Pull requests".into()),
            ..Default::default()
        }
        .checked()
        .unwrap();
        assert_eq!(untitled_app.kind, "window");
        assert_eq!(untitled_app.line(), "Sent from Pull requests");
    }

    #[test]
    fn a_desktop_view_carries_no_window_fields() {
        let v = ViewContext {
            kind: "desktop".into(),
            app: Some("Finder".into()),
            window_id: Some("w1".into()),
            ..Default::default()
        }
        .checked()
        .unwrap();
        assert_eq!(
            v,
            ViewContext {
                kind: "desktop".into(),
                ..Default::default()
            }
        );
        assert_eq!(v.line(), "Sent from the desktop");
    }

    #[test]
    fn a_bad_view_is_refused_and_a_long_title_is_cut() {
        assert!(
            ViewContext {
                kind: "tab".into(),
                ..Default::default()
            }
            .checked()
            .is_err()
        );
        assert!(
            ViewContext {
                kind: "window".into(),
                ..Default::default()
            }
            .checked()
            .is_err()
        );
        assert!(
            window(&"a".repeat(VIEW_APP_MAX + 1), "t", "w")
                .checked()
                .is_err()
        );
        assert!(
            window("a", "t", &"w".repeat(VIEW_WINDOW_ID_MAX + 1))
                .checked()
                .is_err()
        );
        let long = window("Firefox", &"t".repeat(VIEW_TITLE_MAX + 40), "w")
            .checked()
            .unwrap();
        assert_eq!(long.title.unwrap().chars().count(), VIEW_TITLE_MAX);
    }

    #[test]
    fn the_view_is_stored_under_the_proto_field_names() {
        // Drift guard: the JSON names are `ThreadView`'s (cua/env/v1/host.proto).
        let v = window("Firefox", "Pull requests", "w42");
        let json = serde_json::to_value(&v).unwrap();
        let p = cua_proto::env::v1::ThreadView::from(&v);
        assert_eq!(
            json,
            serde_json::json!({"kind": p.kind, "app": p.app, "title": p.title, "window_id": p.window_id})
        );
        assert_eq!(ViewContext::from(p), v);
        let desktop = serde_json::to_value(ViewContext {
            kind: "desktop".into(),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(desktop, serde_json::json!({"kind": "desktop"}));
    }

    #[test]
    fn an_entry_without_a_view_reads_as_before() {
        let e: ThreadEntry = serde_json::from_value(serde_json::json!({
            "v": 1, "seq": 1, "id": "e1", "ts_ms": 1, "kind": "message", "text": "hi",
            "author": {"kind": "person", "id": "owner", "name": "Owner"}
        }))
        .unwrap();
        assert!(e.view.is_none());
        assert!(serde_json::to_value(&e).unwrap().get("view").is_none());
    }
}

/// A Cua Volume file attached to an entry (never inline bytes, never a
/// host path).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mime: String,
}

/// An approval request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Approval {
    pub action: String,
    #[serde(default)]
    pub details: String,
    /// `open` when posted (the answer is a later entry).
    pub state: String,
}

/// An answer to an approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub approval: u64,
    pub approve: bool,
    #[serde(default)]
    pub note: String,
}

/// A task's new state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Update {
    pub task: u64,
    pub state: TaskState,
    #[serde(default)]
    pub session: Option<u32>,
    #[serde(default)]
    pub lease_until_ms: Option<u64>,
    #[serde(default)]
    pub note: String,
}

/// An agent asking to join a Space (an entry of kind `join_request`); the
/// owner or an admin answers it like an approval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinRequest {
    pub member: crate::persistent::identity::Identity,
    /// Who can give it tasks once in: `owner` or `members`.
    pub can_task: String,
    /// `open` when posted (the answer is a later entry).
    pub state: String,
    pub code_id: String,
}

/// Where an imported entry came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Import {
    pub source: String,
    pub id: String,
}

/// Whose provider keys a task's run uses, when someone other than the
/// owner asked for it (no key names, no values).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyOwner {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// One thread entry (format [`VERSION`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThreadEntry {
    pub v: u32,
    pub seq: u64,
    pub id: String,
    pub ts_ms: u64,
    pub author: Author,
    pub kind: EntryKind,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub mentions: Vec<String>,
    #[serde(default)]
    pub reply_to: Option<u64>,
    #[serde(default)]
    pub task: Option<TaskTo>,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub approval: Option<Approval>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Answer>,
    #[serde(default)]
    pub update: Option<Update>,
    /// Imported from an app's earlier conversation (`ts_ms` is the
    /// original time).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub imported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import: Option<Import>,
    /// The task a system note is about (clients show it inside that task).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_seq: Option<u64>,
    /// On a task someone other than the owner gave: the run uses the
    /// owner's provider keys ("runs on Dillon's keys").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runs_on_keys_of: Option<KeyOwner>,
    /// On a system note: what happened, for clients that draw it
    /// (`{kind: member_joined|member_removed|name_refused|rate_limited,
    /// member, by}`); `text` says the same in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_request: Option<JoinRequest>,
    /// What the person was looking at when they posted ([`ViewContext`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewContext>,
    /// On an agent's reply to a task that failed: what failed
    /// (`needs_connect` with the provider to connect, or an error), with
    /// the complete text. `text` says it in plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<cua_agents::failure::Failure>,
    /// Fields of a newer format, kept as they are.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// An entry before the store numbers it.
#[derive(Clone, Debug)]
pub struct Draft {
    pub author: Author,
    pub kind: EntryKind,
    pub text: String,
    pub mentions: Vec<String>,
    pub reply_to: Option<u64>,
    pub task: Option<TaskTo>,
    pub attachments: Vec<Attachment>,
    pub approval: Option<Approval>,
    pub answer: Option<Answer>,
    pub update: Option<Update>,
    /// The entry's id (attachments are stored under it before posting).
    pub id: Option<String>,
    /// An imported entry's original time.
    pub ts_ms: Option<u64>,
    pub import: Option<Import>,
    pub task_seq: Option<u64>,
    pub runs_on_keys_of: Option<KeyOwner>,
    pub event: Option<Value>,
    pub join_request: Option<JoinRequest>,
    pub view: Option<ViewContext>,
    pub failure: Option<cua_agents::failure::Failure>,
}

impl Draft {
    pub fn new(author: Author, kind: EntryKind, text: impl Into<String>) -> Draft {
        Draft {
            author,
            kind,
            text: text.into(),
            mentions: vec![],
            reply_to: None,
            task: None,
            attachments: vec![],
            approval: None,
            answer: None,
            update: None,
            id: None,
            ts_ms: None,
            import: None,
            task_seq: None,
            runs_on_keys_of: None,
            event: None,
            join_request: None,
            view: None,
            failure: None,
        }
    }

    /// A system note with a structured `event` clients draw.
    pub fn event(what: &str, text: impl Into<String>, event: Value) -> Draft {
        let mut d = Draft::new(Author::system(what), EntryKind::System, text);
        d.event = Some(event);
        d
    }

    /// A system note about task `task`.
    pub fn note(what: &str, task: Option<u64>, text: impl Into<String>) -> Draft {
        let mut d = Draft::new(Author::system(what), EntryKind::System, text);
        d.task_seq = task;
        d
    }

    /// A `task_update` entry.
    pub fn update(author: Author, update: Update) -> Draft {
        let mut d = Draft::new(author, EntryKind::TaskUpdate, "");
        d.reply_to = Some(update.task);
        d.update = Some(update);
        d
    }
}

/// A sortable entry id: time first, then randomness.
pub fn new_entry_id() -> String {
    format!(
        "{:012x}{:016x}",
        cua_volume::now_ms(),
        rand::random::<u64>()
    )
}

// ------------------------------------------------------------ the checks

/// The Cua Volume's secret scanner over `text`: a hit refuses the post
/// (`invalid_argument`), naming the kind and line, never the value.
pub fn refuse_secrets(what: &str, text: &str) -> Result<()> {
    match cua_volume::scan::scan(text.as_bytes()).first() {
        Some(f) => Err(Error::invalid(format!(
            "looks like a secret; use the Keyvault ({} on line {} of the {what})",
            f.kind, f.line
        ))),
        None => Ok(()),
    }
}

/// The size limits and the secret scanner for a post's text.
pub fn check_text(text: &str) -> Result<()> {
    if text.len() > MAX_TEXT {
        return Err(Error::invalid(format!(
            "text is {} bytes; at most {MAX_TEXT}",
            text.len()
        )));
    }
    refuse_secrets("text", text)
}

/// `@name` tokens in `text` (lowercased, deduplicated, in order). A name is
/// what an agent name may be (`a-z`, `0-9`, `.`, `_`, `-`); a trailing `.`
/// ends the sentence, not the name.
pub fn parse_mentions(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '@' && (i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_')) {
            let mut j = i + 1;
            let mut name = String::new();
            while j < chars.len() {
                let c = chars[j].to_ascii_lowercase();
                if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                    name.push(c);
                    j += 1;
                } else {
                    break;
                }
            }
            let name = name.trim_end_matches(['.', '-']).to_string();
            if cua_volume::path::valid_agent_name(&name) && !out.contains(&name) {
                out.push(name);
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    out
}

// ----------------------------------------------------------------- store

/// `head.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    /// The next entry's seq (the first is 1).
    #[serde(default = "one")]
    pub next_seq: u64,
    /// The open segment (its first seq).
    #[serde(default = "one")]
    pub segment: u64,
    #[serde(default)]
    pub segment_entries: u64,
    /// Bytes of the open segment that hold whole entries.
    #[serde(default)]
    pub segment_bytes: u64,
    /// Entries up to here are on the Volume.
    #[serde(default)]
    pub mirrored_seq: u64,
    /// Closed segments up to (the first seq of) this one are rolled up on
    /// the Volume.
    #[serde(default)]
    pub rolled_through: u64,
    /// Per-entry Volume objects up to here were deleted (rolled up and
    /// older than a week).
    #[serde(default)]
    pub pruned_through: u64,
    /// `members.json` and `cursors.json` on the Volume: what was written
    /// (a hash) and its etag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members_etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursors_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursors_etag: Option<String>,
}

fn one() -> u64 {
    1
}

impl Head {
    fn fresh() -> Head {
        Head {
            next_seq: 1,
            segment: 1,
            ..Default::default()
        }
    }

    /// The last entry's seq (0: none).
    pub fn last_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }
}

/// A member's read position.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub read_seq: u64,
    pub updated_ms: u64,
}

/// One Space's thread on disk. Cheap to clone; every write takes the
/// thread's file lock, so any process may write.
#[derive(Clone, Debug)]
pub struct ThreadStore {
    dir: PathBuf,
    space: String,
}

fn notifiers() -> &'static Mutex<HashMap<PathBuf, Arc<tokio::sync::Notify>>> {
    static N: OnceLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Notify>>>> = OnceLock::new();
    N.get_or_init(Default::default)
}

fn fold_cache() -> &'static Mutex<HashMap<PathBuf, Fold>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Fold>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn segment_name(first: u64) -> String {
    format!("{first:012}.jsonl")
}

fn io(e: std::io::Error) -> Error {
    Error::from(e)
}

impl ThreadStore {
    /// The thread of `space` (a Space id) under `cua_home`.
    pub fn open(cua_home: &Path, space: &str) -> ThreadStore {
        let folder = cua_volume::path::space_folder_name(space);
        ThreadStore {
            dir: cua_home.join(DIR).join(folder),
            space: space.to_string(),
        }
    }

    /// The thread in `dir` (as [`ThreadStore::list`] finds it).
    fn at(dir: PathBuf) -> Option<ThreadStore> {
        let raw = std::fs::read(dir.join("space.json")).ok()?;
        let v: Value = serde_json::from_slice(&raw).ok()?;
        let space = v.get("space")?.as_str()?.to_string();
        Some(ThreadStore { dir, space })
    }

    /// Every thread under `cua_home`.
    pub fn list(cua_home: &Path) -> Vec<ThreadStore> {
        let Ok(rd) = std::fs::read_dir(cua_home.join(DIR)) else {
            return vec![];
        };
        let mut out: Vec<ThreadStore> = rd
            .flatten()
            .filter_map(|e| ThreadStore::at(e.path()))
            .collect();
        out.sort_by(|a, b| a.dir.cmp(&b.dir));
        out
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The Space id.
    pub fn space(&self) -> &str {
        &self.space
    }

    /// The Space's folder on the Volume (`spaces/<folder>/`).
    pub fn volume_folder(&self) -> String {
        cua_volume::path::space_folder(&self.space)
    }

    /// Whether any entry was ever written here.
    pub fn exists(&self) -> bool {
        self.dir.join("head.json").exists()
    }

    fn log_dir(&self) -> PathBuf {
        self.dir.join("log")
    }

    fn notifier(&self) -> Arc<tokio::sync::Notify> {
        notifiers()
            .lock()
            .expect("notifiers")
            .entry(self.dir.clone())
            .or_default()
            .clone()
    }

    /// Takes the thread's write lock (blocking; held only for one write).
    fn lock(&self) -> Result<std::fs::File> {
        std::fs::create_dir_all(self.log_dir()).map_err(io)?;
        let path = self.dir.join("thread.lock");
        cua_home::guard_write(&path).map_err(io)?;
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(io)?;
        f.lock().map_err(io)?;
        let space = self.dir.join("space.json");
        if !space.exists() {
            let v = serde_json::json!({"space": self.space});
            cua_home::write_private(&space, v.to_string().as_bytes()).map_err(io)?;
        }
        Ok(f)
    }

    /// The head as written.
    pub fn head(&self) -> Result<Head> {
        match std::fs::read(self.dir.join("head.json")) {
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Head::fresh()),
            Err(e) => Err(io(e)),
        }
    }

    fn write_head(&self, head: &Head) -> Result<()> {
        cua_home::write_private(
            &self.dir.join("head.json"),
            &serde_json::to_vec_pretty(head)?,
        )
        .map_err(io)
    }

    /// Read-modify-write of the head under the lock (the mirror's
    /// bookkeeping).
    pub fn update_head<R>(&self, f: impl FnOnce(&mut Head) -> R) -> Result<R> {
        let _l = self.lock()?;
        let mut head = self.recovered_head()?;
        let r = f(&mut head);
        self.write_head(&head)?;
        Ok(r)
    }

    /// The head, with entries a crash wrote after the last head write
    /// counted, and a torn last line dropped. Call with the lock held.
    fn recovered_head(&self) -> Result<Head> {
        let mut head = self.head()?;
        let path = self.log_dir().join(segment_name(head.segment));
        let Ok(mut f) = OpenOptions::new().read(true).write(true).open(&path) else {
            return Ok(head);
        };
        let len = f.metadata().map_err(io)?.len();
        if len <= head.segment_bytes {
            return Ok(head);
        }
        let mut tail = vec![];
        f.seek(SeekFrom::Start(head.segment_bytes)).map_err(io)?;
        f.read_to_end(&mut tail).map_err(io)?;
        let whole = tail.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
        for line in tail[..whole].split(|b| *b == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            if let Ok(e) = serde_json::from_slice::<ThreadEntry>(line)
                && e.seq >= head.next_seq
            {
                head.next_seq = e.seq + 1;
                head.segment_entries += 1;
            }
        }
        head.segment_bytes += whole as u64;
        if (whole as u64) < tail.len() as u64 {
            f.set_len(head.segment_bytes).map_err(io)?;
        }
        Ok(head)
    }

    /// Appends `drafts` in order and returns them as written.
    pub fn append_many(&self, drafts: Vec<Draft>) -> Result<Vec<ThreadEntry>> {
        if drafts.is_empty() {
            return Ok(vec![]);
        }
        let _l = self.lock()?;
        self.append_locked(drafts)
    }

    /// [`Self::append_many`] with the lock already held.
    fn append_locked(&self, drafts: Vec<Draft>) -> Result<Vec<ThreadEntry>> {
        if drafts.is_empty() {
            return Ok(vec![]);
        }
        let mut head = self.recovered_head()?;
        let mut out = vec![];
        for d in drafts {
            let e = ThreadEntry {
                v: VERSION,
                seq: head.next_seq,
                id: d.id.unwrap_or_else(new_entry_id),
                ts_ms: d.ts_ms.unwrap_or_else(cua_volume::now_ms),
                author: d.author,
                kind: d.kind,
                text: d.text,
                mentions: d.mentions,
                reply_to: d.reply_to,
                task: d.task,
                attachments: d.attachments,
                approval: d.approval,
                answer: d.answer,
                update: d.update,
                imported: d.import.is_some(),
                import: d.import,
                task_seq: d.task_seq,
                runs_on_keys_of: d.runs_on_keys_of,
                event: d.event,
                join_request: d.join_request,
                view: d.view,
                failure: d.failure,
                extra: Map::new(),
            };
            self.write_entry(&mut head, &e)?;
            out.push(e);
        }
        self.write_head(&head)?;
        self.notifier().notify_waiters();
        Ok(out)
    }

    /// Appends one entry.
    pub fn append(&self, draft: Draft) -> Result<ThreadEntry> {
        Ok(self.append_many(vec![draft])?.remove(0))
    }

    /// Writes `e` (numbered already) at the end of the log. Lock held.
    fn write_entry(&self, head: &mut Head, e: &ThreadEntry) -> Result<()> {
        if head.segment_entries >= SEGMENT_ENTRIES || head.segment_bytes >= SEGMENT_BYTES {
            head.segment = e.seq;
            head.segment_entries = 0;
            head.segment_bytes = 0;
        }
        let mut line = serde_json::to_vec(e)?;
        line.push(b'\n');
        let path = self.log_dir().join(segment_name(head.segment));
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(io)?;
        f.write_all(&line).map_err(io)?;
        f.sync_data().map_err(io)?;
        head.segment_entries += 1;
        head.segment_bytes += line.len() as u64;
        head.next_seq = e.seq + 1;
        Ok(())
    }

    /// Replays entries (from the Volume) into an empty log, keeping their
    /// seqs. Entries at or below the current last seq are skipped.
    pub fn replay(&self, mut entries: Vec<ThreadEntry>) -> Result<usize> {
        entries.sort_by_key(|e| e.seq);
        let _l = self.lock()?;
        let mut head = self.recovered_head()?;
        let mut n = 0;
        for e in entries {
            if e.seq < head.next_seq {
                continue;
            }
            self.write_entry(&mut head, &e)?;
            n += 1;
        }
        head.mirrored_seq = head.last_seq();
        self.write_head(&head)?;
        fold_cache().lock().expect("fold cache").remove(&self.dir);
        self.notifier().notify_waiters();
        Ok(n)
    }

    /// The segments (first seqs), oldest first.
    pub fn segments(&self) -> Vec<u64> {
        let Ok(rd) = std::fs::read_dir(self.log_dir()) else {
            return vec![];
        };
        let mut v: Vec<u64> = rd
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".jsonl")?.parse().ok())
            .collect();
        v.sort_unstable();
        v
    }

    /// Every entry of the segment starting at `first`.
    pub fn read_segment(&self, first: u64) -> Result<Vec<ThreadEntry>> {
        let raw = match std::fs::read(self.log_dir().join(segment_name(first))) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(io(e)),
        };
        Ok(raw
            .split(|b| *b == b'\n')
            .filter(|l| !l.iter().all(u8::is_ascii_whitespace))
            .filter_map(|l| serde_json::from_slice(l).ok())
            .collect())
    }

    /// Up to `limit` entries after `after`, in order.
    pub fn read(&self, after: u64, limit: usize) -> Result<Vec<ThreadEntry>> {
        let segs = self.segments();
        let mut out = vec![];
        for (i, first) in segs.iter().enumerate() {
            // Skip segments that end at or before `after`.
            if let Some(next) = segs.get(i + 1)
                && *next <= after + 1
            {
                continue;
            }
            for e in self.read_segment(*first)? {
                if e.seq > after {
                    out.push(e);
                    if out.len() >= limit {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Up to `limit` entries before `before` (default: the newest), in
    /// order, and whether older ones exist.
    pub fn read_before(
        &self,
        before: Option<u64>,
        limit: usize,
    ) -> Result<(Vec<ThreadEntry>, bool)> {
        let bound = before.unwrap_or(self.last_seq()? + 1);
        let lo = bound.saturating_sub(limit as u64 + 1);
        let mut page: Vec<ThreadEntry> = self
            .read(lo, usize::MAX)?
            .into_iter()
            .take_while(|e| e.seq < bound)
            .collect();
        let extra = page.len().saturating_sub(limit);
        page.drain(..extra);
        let more = match page.first() {
            Some(first) => self.read(0, 1)?.first().is_some_and(|e| e.seq < first.seq),
            None => false,
        };
        Ok((page, more))
    }

    /// One entry.
    pub fn get(&self, seq: u64) -> Result<Option<ThreadEntry>> {
        Ok(self
            .read(seq.saturating_sub(1), 1)?
            .into_iter()
            .find(|e| e.seq == seq))
    }

    /// The last seq written (0: none).
    pub fn last_seq(&self) -> Result<u64> {
        Ok(self.head()?.last_seq())
    }

    /// Waits up to `for_` for an entry after `after` (in-process writers
    /// wake it at once; other processes' writes are seen within 250 ms).
    pub async fn wait_after(&self, after: u64, for_: Duration) -> Result<()> {
        self.wait_after_or(after, for_, || false).await
    }

    /// Wakes the readers that follow this thread (what they show besides
    /// its entries changed: an agent's live stage).
    pub fn wake(&self) {
        self.notifier().notify_waiters();
    }

    /// [`Self::wait_after`], also returning as soon as `changed` says so.
    pub async fn wait_after_or(
        &self,
        after: u64,
        for_: Duration,
        changed: impl Fn() -> bool,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + for_.min(MAX_FOLLOW);
        let n = self.notifier();
        loop {
            let notified = n.notified();
            if self.last_seq()? > after || changed() {
                return Ok(());
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Ok(());
            }
            let step = (deadline - now).min(Duration::from_millis(250));
            tokio::select! {
                _ = notified => {}
                _ = tokio::time::sleep(step) => {}
            }
        }
    }

    // --- cursors

    /// Every member's read position.
    pub fn cursors(&self) -> Result<BTreeMap<String, Cursor>> {
        match std::fs::read(self.dir.join("cursors.json")) {
            Ok(b) if b.iter().all(u8::is_ascii_whitespace) => Ok(BTreeMap::new()),
            Ok(b) => Ok(serde_json::from_slice(&b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(io(e)),
        }
    }

    /// Moves `member`'s read position to `seq` (never backwards).
    pub fn mark_read(&self, member: &str, seq: u64) -> Result<Cursor> {
        let _l = self.lock()?;
        let mut all = self.cursors()?;
        let c = all.entry(member.to_string()).or_default();
        if seq > c.read_seq {
            c.read_seq = seq;
            c.updated_ms = cua_volume::now_ms();
        }
        let out = c.clone();
        cua_home::write_private(
            &self.dir.join("cursors.json"),
            &serde_json::to_vec_pretty(&all)?,
        )
        .map_err(io)?;
        Ok(out)
    }

    /// Entries after `member`'s read position that someone else wrote.
    pub fn unread(&self, member: &str) -> Result<u64> {
        let read = self.cursors()?.get(member).map(|c| c.read_seq).unwrap_or(0);
        Ok(self
            .read(read, usize::MAX)?
            .iter()
            .filter(|e| e.author.id != member)
            .count() as u64)
    }

    // --- the fold

    /// The tasks, approvals and imports folded from every entry (cached
    /// per thread; only new entries are read).
    pub fn fold(&self) -> Result<Fold> {
        let cache = fold_cache();
        let last = self.last_seq()?;
        let mut fold = cache
            .lock()
            .expect("fold cache")
            .get(&self.dir)
            .cloned()
            .filter(|f| f.through <= last)
            .unwrap_or_default();
        if fold.through < last {
            for e in self.read(fold.through, usize::MAX)? {
                fold.apply(&e);
            }
            cache
                .lock()
                .expect("fold cache")
                .insert(self.dir.clone(), fold.clone());
        }
        Ok(fold)
    }
}

/// The DM between person `me` (an author id) and agent member `with` (its
/// handle), as a view of the thread (CUA-2266): the agent's entries, mine
/// that task or @mention it, and everything that hangs off those (replies,
/// task updates, answers, notes about them, notes about the member). Oldest
/// first; one forward pass, so an entry joins only through earlier ones.
pub fn dm_entries(entries: &[ThreadEntry], me: &str, with: &str) -> Vec<ThreadEntry> {
    let agent = format!("agent:{with}");
    let mut seqs = BTreeSet::new();
    let mut out = vec![];
    for e in entries {
        let joined = |s: Option<u64>| s.is_some_and(|s| seqs.contains(&s));
        let about_member = e
            .event
            .as_ref()
            .and_then(|v| v.pointer("/member/handle"))
            .and_then(Value::as_str)
            == Some(with);
        let keep = e.author.id == agent
            || (e.author.id == me
                && (e.task.as_ref().is_some_and(|t| t.to == with)
                    || e.mentions.iter().any(|m| m == with)))
            || joined(e.reply_to)
            || joined(e.update.as_ref().map(|u| u.task))
            || joined(e.answer.as_ref().map(|a| a.approval))
            || (e.kind == EntryKind::System && (joined(e.task_seq) || about_member));
        if keep {
            seqs.insert(e.seq);
            out.push(e.clone());
        }
    }
    out
}

/// The read-cursor key of `me`'s DM with agent `with` (kept apart from the
/// thread's own cursor, so reading one doesn't mark the other).
pub fn dm_cursor_key(me: &str, with: &str) -> String {
    format!("{me}\u{0}dm:{with}")
}

/// A person's @mention handle: their name, lower-cased, without spaces
/// (what the composer's picker writes).
pub fn mention_handle(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// What the Agents tab shows for one agent in one thread (CUA-2266).
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DmSummary {
    /// DM entries after `me`'s DM cursor that someone else wrote.
    pub unread: u64,
    /// The DM's latest entry by a person or the agent.
    pub last: Option<DmLast>,
    /// What waits for `me`: open approvals `me` may answer, and the agent's
    /// latest @mention of `me` when `me` hasn't written in the DM since.
    pub needs_you: Vec<NeedsYou>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DmLast {
    pub seq: u64,
    pub ts_ms: u64,
    pub text: String,
    pub author: Author,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct NeedsYou {
    pub seq: u64,
    /// `approval` or `question`.
    pub kind: &'static str,
    pub text: String,
    pub ts_ms: u64,
}

/// The DM summary of agent `with` for person `me` (`me_name` gives its
/// @mention handle). `read` is `me`'s DM cursor; `approvals` the thread's
/// approvals and whether each was answered; `may_approve` whether `me`
/// holds the agent's approve right.
pub fn dm_summary(
    entries: &[ThreadEntry],
    me: &str,
    me_name: &str,
    with: &str,
    read: u64,
    approvals: &BTreeMap<u64, bool>,
    may_approve: bool,
) -> DmSummary {
    let dm = dm_entries(entries, me, with);
    let agent = format!("agent:{with}");
    let unread = dm
        .iter()
        .filter(|e| e.seq > read && e.author.id != me)
        .count() as u64;
    let last = dm
        .iter()
        .rev()
        .find(|e| matches!(e.kind, EntryKind::Message | EntryKind::Approval))
        .map(|e| DmLast {
            seq: e.seq,
            ts_ms: e.ts_ms,
            text: clip(&e.text, 200),
            author: e.author.clone(),
        });
    let mut needs_you = vec![];
    if may_approve {
        for e in dm
            .iter()
            .filter(|e| e.author.id == agent && e.kind == EntryKind::Approval)
        {
            if approvals.get(&e.seq) == Some(&false) {
                let text = e
                    .approval
                    .as_ref()
                    .map_or_else(|| e.text.clone(), |a| a.action.clone());
                needs_you.push(NeedsYou {
                    seq: e.seq,
                    kind: "approval",
                    text: clip(&text, 200),
                    ts_ms: e.ts_ms,
                });
            }
        }
    }
    let handle = mention_handle(me_name);
    let mine_last = dm
        .iter()
        .rev()
        .find(|e| e.author.id == me)
        .map_or(0, |e| e.seq);
    if let Some(q) = dm.iter().rev().find(|e| {
        e.author.id == agent && e.kind == EntryKind::Message && e.mentions.contains(&handle)
    }) && q.seq > mine_last
    {
        needs_you.push(NeedsYou {
            seq: q.seq,
            kind: "question",
            text: clip(&q.text, 200),
            ts_ms: q.ts_ms,
        });
    }
    needs_you.sort_by_key(|n| n.seq);
    DmSummary {
        unread,
        last,
        needs_you,
    }
}

/// One task, as the entries so far say.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TaskInfo {
    /// The task's entry.
    pub seq: u64,
    /// The agent member it is for.
    pub to: String,
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<u32>,
    /// The session the task was addressed to (`to_session`), if one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_session: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_until_ms: Option<u64>,
    pub author: Author,
    pub ts_ms: u64,
    /// The first 200 characters.
    pub text: String,
    /// The agent replied under it itself.
    #[serde(skip)]
    pub replied: bool,
    /// The claim's holder (`<member>/<session>@<epoch>`) while one holds
    /// it ([`ThreadStore::with_claims`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub holder: Option<String>,
    /// What the person who gave it was looking at ([`ViewContext`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewContext>,
}

/// What the entries add up to.
#[derive(Clone, Debug, Default)]
pub struct Fold {
    /// The last seq folded.
    pub through: u64,
    pub tasks: BTreeMap<u64, TaskInfo>,
    /// Import ids already imported.
    pub imports: HashSet<String>,
    /// Approval (and join request) entries and whether they were answered.
    pub approvals: BTreeMap<u64, bool>,
    /// Join request entries and how they were answered (`None`: open).
    pub join_requests: BTreeMap<u64, Option<bool>>,
    /// Names of the people who wrote in the thread.
    pub people: BTreeSet<String>,
    /// Who asked each approval, and the entry it answers to (an agent's
    /// approval under its task makes that task's session wait).
    pub approval_asked: BTreeMap<u64, (Author, Option<u64>)>,
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

impl Fold {
    fn apply(&mut self, e: &ThreadEntry) {
        self.through = self.through.max(e.seq);
        if let Some(i) = &e.import {
            self.imports.insert(format!("{}\u{0}{}", i.source, i.id));
        }
        if let Some(t) = &e.task
            && !e.imported
        {
            self.tasks.insert(
                e.seq,
                TaskInfo {
                    seq: e.seq,
                    to: t.to.clone(),
                    state: TaskState::Queued,
                    session: t.session,
                    to_session: t.session,
                    lease_until_ms: None,
                    author: e.author.clone(),
                    ts_ms: e.ts_ms,
                    text: clip(&e.text, 200),
                    replied: false,
                    holder: None,
                    view: e.view.clone(),
                },
            );
        }
        if e.approval.is_some() || e.join_request.is_some() {
            self.approvals.entry(e.seq).or_insert(false);
            self.approval_asked
                .insert(e.seq, (e.author.clone(), e.reply_to));
        }
        if e.join_request.is_some() {
            self.join_requests.entry(e.seq).or_insert(None);
        }
        if let Some(a) = &e.answer
            && let Some(answered) = self.approvals.get_mut(&a.approval)
        {
            *answered = true;
            if let Some(j) = self.join_requests.get_mut(&a.approval) {
                *j = Some(a.approve);
            }
        }
        if e.author.kind == AuthorKind::Person && !e.author.name.trim().is_empty() {
            self.people.insert(e.author.name.clone());
        }
        if let Some(u) = &e.update
            && let Some(t) = self.tasks.get_mut(&u.task)
        {
            t.state = u.state;
            if u.state == TaskState::Queued {
                // Back in the queue: for any session again, or the one it
                // was addressed to.
                t.session = t.to_session;
            } else if u.session.is_some() {
                t.session = u.session;
            }
            t.lease_until_ms = u.lease_until_ms;
        }
        if e.kind == EntryKind::Message
            && let Some(r) = e.reply_to
            && let Some(t) = self.tasks.get_mut(&r)
            && e.author.agent_name() == Some(t.to.as_str())
        {
            t.replied = true;
        }
    }

    /// The approval `member` asked under task `task` that nobody answered
    /// yet, if any.
    pub fn open_approval_under(&self, member: &str, task: u64) -> Option<u64> {
        self.approval_asked
            .iter()
            .find(|(seq, (author, reply_to))| {
                *reply_to == Some(task)
                    && author.agent_name() == Some(member)
                    && self.approvals.get(seq) == Some(&false)
            })
            .map(|(seq, _)| *seq)
    }

    /// Whether `source`/`id` was imported.
    pub fn imported(&self, source: &str, id: &str) -> bool {
        self.imports.contains(&format!("{source}\u{0}{id}"))
    }

    /// Open tasks (not done or failed), oldest first, of `member` (or
    /// everyone).
    pub fn open_tasks(&self, member: Option<&str>) -> Vec<TaskInfo> {
        self.tasks
            .values()
            .filter(|t| !t.state.is_closed())
            .filter(|t| member.is_none_or(|m| t.to == m))
            .cloned()
            .collect()
    }

    /// The oldest queued task of `member`.
    pub fn next_queued(&self, member: &str) -> Option<TaskInfo> {
        self.tasks
            .values()
            .find(|t| t.to == member && t.state == TaskState::Queued)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, ThreadStore) {
        let d = tempfile::tempdir().unwrap();
        let s = ThreadStore::open(d.path(), "local:dev");
        (d, s)
    }

    fn msg(text: &str) -> Draft {
        Draft::new(Author::owner(), EntryKind::Message, text)
    }

    #[test]
    fn mentions_parse_names_not_emails() {
        assert_eq!(
            parse_mentions("@claude run it, then @Codex. cc a@b.com and @x"),
            vec!["claude", "codex", "x"]
        );
        assert_eq!(parse_mentions("@re.view-1."), vec!["re.view-1"]);
        assert!(parse_mentions("@ @-").is_empty());
    }

    #[test]
    fn appends_number_from_one_and_read_after_a_cursor() {
        let (_d, s) = store();
        assert!(!s.exists());
        for i in 0..5 {
            assert_eq!(s.append(msg(&format!("m{i}"))).unwrap().seq, i + 1);
        }
        let page = s.read(2, 2).unwrap();
        assert_eq!(page.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![3, 4]);
        assert_eq!(s.last_seq().unwrap(), 5);
        assert_eq!(s.get(4).unwrap().unwrap().text, "m3");
        let again = ThreadStore::list(_d.path());
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].space(), "local:dev");
    }

    #[test]
    fn segments_roll_and_reads_span_them() {
        let (_d, s) = store();
        // Force small segments by writing a head that is "full".
        s.append(msg("a")).unwrap();
        s.update_head(|h| h.segment_entries = SEGMENT_ENTRIES)
            .unwrap();
        s.append(msg("b")).unwrap();
        s.append(msg("c")).unwrap();
        assert_eq!(s.segments(), vec![1, 2]);
        let all = s.read(0, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(s.read(1, 10).unwrap()[0].text, "b");
    }

    #[test]
    fn a_crash_between_log_and_head_is_recovered_and_a_torn_line_dropped() {
        let (_d, s) = store();
        s.append(msg("a")).unwrap();
        let head = s.head().unwrap();
        s.append(msg("b")).unwrap();
        // The head as it was before "b" (the crash lost the head write),
        // and half a line after "b".
        cua_home::write_private(
            &s.dir().join("head.json"),
            &serde_json::to_vec(&head).unwrap(),
        )
        .unwrap();
        let seg = s.dir().join("log").join(segment_name(1));
        let mut f = OpenOptions::new().append(true).open(&seg).unwrap();
        f.write_all(b"{\"v\":1,\"seq\":3,").unwrap();
        let c = s.append(msg("c")).unwrap();
        assert_eq!(c.seq, 3);
        let texts: Vec<String> = s.read(0, 10).unwrap().into_iter().map(|e| e.text).collect();
        assert_eq!(texts, vec!["a", "b", "c"]);
    }

    #[test]
    fn tasks_fold_from_updates() {
        let (_d, s) = store();
        let mut d = msg("@claude go");
        d.task = Some(TaskTo {
            to: "claude".into(),
            session: None,
        });
        let t = s.append(d).unwrap();
        assert_eq!(s.fold().unwrap().next_queued("claude").unwrap().seq, t.seq);
        for state in [TaskState::Claimed, TaskState::Working] {
            s.append(Draft::update(
                Author::system("dispatch"),
                Update {
                    task: t.seq,
                    state,
                    session: Some(1),
                    lease_until_ms: None,
                    note: String::new(),
                },
            ))
            .unwrap();
        }
        let f = s.fold().unwrap();
        assert_eq!(f.tasks[&t.seq].state, TaskState::Working);
        assert!(f.next_queued("claude").is_none());
        assert_eq!(f.open_tasks(Some("claude")).len(), 1);
        let mut r = Draft::new(Author::agent("claude"), EntryKind::Message, "done");
        r.reply_to = Some(t.seq);
        s.append(r).unwrap();
        assert!(s.fold().unwrap().tasks[&t.seq].replied);
        s.append(Draft::update(
            Author::agent("claude"),
            Update {
                task: t.seq,
                state: TaskState::Done,
                session: None,
                lease_until_ms: None,
                note: String::new(),
            },
        ))
        .unwrap();
        assert!(s.fold().unwrap().open_tasks(None).is_empty());
    }

    #[test]
    fn cursors_count_unread_from_others_only() {
        let (_d, s) = store();
        s.append(msg("1")).unwrap();
        s.append(Draft::new(Author::agent("claude"), EntryKind::Message, "2"))
            .unwrap();
        s.append(msg("3")).unwrap();
        assert_eq!(s.unread("owner").unwrap(), 1);
        assert_eq!(s.unread("agent:claude").unwrap(), 2);
        s.mark_read("agent:claude", 2).unwrap();
        assert_eq!(s.unread("agent:claude").unwrap(), 1);
        // Never backwards.
        assert_eq!(s.mark_read("agent:claude", 1).unwrap().read_seq, 2);
    }

    #[test]
    fn secrets_and_size_are_refused() {
        let e = check_text("key: ghp_0123456789abcdefghijklmnopqrstuvwxyzAB").unwrap_err();
        assert_eq!(e.tag(), "invalid_argument");
        assert!(
            e.to_string()
                .contains("looks like a secret; use the Keyvault")
        );
        assert!(!e.to_string().contains("ghp_"));
        let e = check_text(&"x".repeat(MAX_TEXT + 1)).unwrap_err();
        assert_eq!(e.tag(), "invalid_argument");
    }

    #[test]
    fn unknown_entry_fields_and_kinds_survive() {
        let line = r#"{"v":2,"seq":1,"id":"x","ts_ms":1,"author":{"kind":"person","id":"owner","name":"O"},"kind":"poll","text":"hi","future":{"a":1}}"#;
        let e: ThreadEntry = serde_json::from_str(line).unwrap();
        assert_eq!(e.kind, EntryKind::Other);
        assert_eq!(e.extra["future"]["a"], 1);
    }

    fn dm_fixture() -> (tempfile::TempDir, ThreadStore) {
        let (d, s) = store();
        let mut task = msg("@codex update the snapshots");
        task.mentions = vec!["codex".into()];
        task.task = Some(TaskTo {
            to: "codex".into(),
            session: None,
        });
        s.append(task).unwrap(); // 1
        let mut other = msg("@hermes write the notes");
        other.mentions = vec!["hermes".into()];
        other.task = Some(TaskTo {
            to: "hermes".into(),
            session: None,
        });
        s.append(other).unwrap(); // 2: not the DM
        let mut up = Draft::new(Author::system("dispatcher"), EntryKind::TaskUpdate, "");
        up.update = Some(Update {
            task: 1,
            state: TaskState::Done,
            session: None,
            lease_until_ms: None,
            note: String::new(),
        });
        s.append(up).unwrap(); // 3
        let mut reply = Draft::new(
            Author::agent("codex"),
            EntryKind::Message,
            "Updated 14 snapshots.",
        );
        reply.reply_to = Some(1);
        s.append(reply).unwrap(); // 4
        s.append(Draft::new(
            Author::agent("hermes"),
            EntryKind::Message,
            "notes done",
        ))
        .unwrap(); // 5: not the DM
        let mut ask = Draft::new(
            Author::agent("codex"),
            EntryKind::Message,
            "@owner which branch, main or grid-v2?",
        );
        ask.mentions = vec!["owner".into()];
        s.append(ask).unwrap(); // 6
        let mut appr = Draft::new(Author::agent("codex"), EntryKind::Approval, "Can I push?");
        appr.approval = Some(Approval {
            action: "Push to main".into(),
            details: String::new(),
            state: "open".into(),
        });
        s.append(appr).unwrap(); // 7
        s.append(Draft::event(
            "members",
            "Codex joined.",
            serde_json::json!({"kind": "member_joined", "member": {"handle": "codex"}}),
        ))
        .unwrap(); // 8
        s.append(Draft::event(
            "members",
            "Hermes joined.",
            serde_json::json!({"kind": "member_joined", "member": {"handle": "hermes"}}),
        ))
        .unwrap(); // 9: not
        (d, s)
    }

    #[test]
    fn a_dm_is_the_agents_entries_with_me_and_what_hangs_off_them() {
        let (_d, s) = dm_fixture();
        let all = s.read(0, usize::MAX).unwrap();
        let seqs: Vec<u64> = dm_entries(&all, "owner", "codex")
            .iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(seqs, vec![1, 3, 4, 6, 7, 8]);
        let seqs: Vec<u64> = dm_entries(&all, "owner", "hermes")
            .iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(seqs, vec![2, 5, 9]);
        // Someone else's DM with codex holds only codex's own entries.
        let seqs: Vec<u64> = dm_entries(&all, "acct_grace", "codex")
            .iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(seqs, vec![4, 6, 7, 8]);
    }

    #[test]
    fn a_dm_summary_counts_unread_and_what_waits_for_me() {
        let (_d, s) = dm_fixture();
        let all = s.read(0, usize::MAX).unwrap();
        let fold = s.fold().unwrap();
        let sum = dm_summary(&all, "owner", "Owner", "codex", 0, &fold.approvals, true);
        // My own task isn't unread; everything else in the DM is.
        assert_eq!(sum.unread, 5);
        assert_eq!(sum.last.as_ref().map(|l| l.seq), Some(7));
        assert_eq!(
            sum.needs_you
                .iter()
                .map(|n| (n.seq, n.kind))
                .collect::<Vec<_>>(),
            vec![(6, "question"), (7, "approval")]
        );
        assert_eq!(sum.needs_you[1].text, "Push to main");
        // Read up to 6, and without the approve right: one unread, only the question.
        let sum = dm_summary(&all, "owner", "Owner", "codex", 6, &fold.approvals, false);
        assert_eq!(sum.unread, 2);
        assert_eq!(
            sum.needs_you.iter().map(|n| n.kind).collect::<Vec<_>>(),
            vec!["question"]
        );
        // Once I write in the DM, the question is answered.
        let mut mine = msg("@codex main");
        mine.mentions = vec!["codex".into()];
        s.append(mine).unwrap();
        let all = s.read(0, usize::MAX).unwrap();
        let sum = dm_summary(
            &all,
            "owner",
            "Owner",
            "codex",
            0,
            &s.fold().unwrap().approvals,
            false,
        );
        assert!(sum.needs_you.is_empty());
        assert_eq!(mention_handle("Ada Lovelace"), "adalovelace");
        assert_eq!(dm_cursor_key("acct_ada", "codex"), "acct_ada\u{0}dm:codex");
    }

    #[tokio::test]
    async fn a_follow_wakes_on_a_new_entry() {
        let (_d, s) = store();
        let s2 = s.clone();
        let t = tokio::spawn(async move {
            let t0 = std::time::Instant::now();
            s2.wait_after(0, Duration::from_secs(10)).await.unwrap();
            t0.elapsed()
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        s.append(msg("hi")).unwrap();
        assert!(t.await.unwrap() < Duration::from_secs(5));
    }
}
