//! The coding agents' own recent chats on this computer, read-only:
//! Claude Code (`~/.claude/projects`), Codex (`~/.codex/sessions`) and
//! Hermes (`~/.hermes/state.db`).
//!
//! [`recent`] lists the newest chats with only what a picker shows: a title
//! (or the first prompt, cut short), the working folder and the last
//! activity. [`history`] reads one chat's messages (what the person and the
//! agent said, no tool calls or outputs) once the person picked it.
//!
//! Nothing here writes or sends anything, and the only thing kept is
//! [`recent_cached`]'s listing, in memory for a few seconds. Each agent's store stays
//! the source of truth, and every format is read defensively: a record,
//! column or line this crate does not know is skipped, a file it cannot read
//! is left out. Large files are read from their head and tail only when
//! listing, and a chat's messages are read from the end of its file, within a
//! byte and time budget, so a chat of any size opens at once (older messages
//! are left unread: `earlier_omitted` is then at least 1).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod claude;
mod codex;
mod hermes;
mod lines;
mod text;

pub use text::{TITLE_MAX, truncate_chars};

/// A message in [`history`] is cut to this many bytes (on a char boundary).
pub const MESSAGE_MAX_BYTES: usize = 8 * 1024;
/// [`history`] keeps at most this many of the newest messages by default.
pub const HISTORY_DEFAULT: usize = 200;
/// [`recent`] lists at most this many chats.
pub const RECENT_MAX: usize = 50;

/// A coding agent whose chats this crate reads. The ids are the harness ids
/// `agent_start` takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Agent {
    #[serde(rename = "claude-code")]
    ClaudeCode,
    #[serde(rename = "openai-codex", alias = "codex")]
    Codex,
    #[serde(rename = "hermes")]
    Hermes,
}

impl Agent {
    pub const ALL: [Agent; 3] = [Agent::ClaudeCode, Agent::Codex, Agent::Hermes];

    /// The harness id (`claude-code`, `openai-codex`, `hermes`).
    pub fn id(self) -> &'static str {
        match self {
            Agent::ClaudeCode => "claude-code",
            Agent::Codex => "openai-codex",
            Agent::Hermes => "hermes",
        }
    }

    /// Parses a harness id or the agent-setup id (`codex`).
    pub fn parse(s: &str) -> Option<Agent> {
        match s {
            "claude-code" | "claude" => Some(Agent::ClaudeCode),
            "openai-codex" | "codex" => Some(Agent::Codex),
            "hermes" => Some(Agent::Hermes),
            _ => None,
        }
    }

    /// The agent's own command that resumes chat `id` in a terminal (run it
    /// in the chat's folder).
    pub fn resume_command(self, id: &str) -> Vec<String> {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        match self {
            Agent::ClaudeCode => v(&["claude", "--resume", id]),
            Agent::Codex => v(&["codex", "resume", id]),
            Agent::Hermes => v(&["hermes", "--resume", id]),
        }
    }
}

/// Where each agent keeps its chats. `None` skips that agent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roots {
    /// Claude Code's config folder (`~/.claude`, or `$CLAUDE_CONFIG_DIR`).
    pub claude: Option<PathBuf>,
    /// Codex's home (`~/.codex`, or `$CODEX_HOME`).
    pub codex: Option<PathBuf>,
    /// Hermes's home (`~/.hermes`, or `$HERMES_HOME`).
    pub hermes: Option<PathBuf>,
}

impl Roots {
    /// The default folders under `home`.
    pub fn under_home(home: &Path) -> Roots {
        Roots {
            claude: Some(home.join(".claude")),
            codex: Some(home.join(".codex")),
            hermes: Some(home.join(".hermes")),
        }
    }

    /// This user's folders: each agent's own variable when set, otherwise
    /// the default under the home folder.
    pub fn from_env() -> Roots {
        let home = home_dir();
        let pick = |var: &str, default: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|h| h.join(default)))
        };
        Roots {
            claude: pick("CLAUDE_CONFIG_DIR", ".claude"),
            codex: pick("CODEX_HOME", ".codex"),
            hermes: pick("HERMES_HOME", ".hermes"),
        }
    }

    fn root(&self, agent: Agent) -> Option<&Path> {
        match agent {
            Agent::ClaudeCode => self.claude.as_deref(),
            Agent::Codex => self.codex.as_deref(),
            Agent::Hermes => self.hermes.as_deref(),
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// One chat, as a picker lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadSummary {
    pub agent: Agent,
    /// The agent's own session id.
    pub id: String,
    /// The chat's title, or its first prompt, cut to [`TITLE_MAX`]
    /// characters on one line.
    pub title: String,
    /// The folder the chat worked in, when the store records one.
    pub cwd: Option<String>,
    /// The last activity (Unix ms).
    pub last_active_ms: u64,
    /// The agent's own command that resumes it in a terminal.
    pub resume: Vec<String>,
}

/// Who said a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The person (this computer's user).
    Person,
    /// The agent.
    Agent,
}

/// One message of a chat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// A stable id within the chat (the store's own record id when it has one).
    pub key: String,
    pub role: Role,
    /// The text, cut to [`MESSAGE_MAX_BYTES`].
    pub text: String,
    /// When it was said (Unix ms), when the store records it.
    pub ts_ms: Option<u64>,
    /// The text was cut.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
}

/// One chat's messages, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadHistory {
    pub thread: ThreadSummary,
    pub messages: Vec<Message>,
    /// Older messages were left out (more than the limit).
    pub earlier_omitted: usize,
}

/// What [`recent`] lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListOptions {
    /// The agents to look at (default: all three).
    pub agents: Vec<Agent>,
    /// How many chats in all, newest first (at most [`RECENT_MAX`]).
    pub limit: usize,
}

impl Default for ListOptions {
    fn default() -> Self {
        ListOptions {
            agents: Agent::ALL.to_vec(),
            limit: 4,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no {agent} chat {id} on this computer")]
    NotFound { agent: &'static str, id: String },
    #[error("{0}")]
    Invalid(String),
    #[error("couldn't read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("couldn't read Hermes's sessions: {0}")]
    Sqlite(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// The newest chats of the agents in `opts`, newest first. An agent whose
/// store is missing or unreadable adds nothing.
pub fn recent(roots: &Roots, opts: &ListOptions) -> Vec<ThreadSummary> {
    let limit = opts.limit.clamp(1, RECENT_MAX);
    let mut agents = opts.agents.clone();
    agents.sort_by_key(|a| a.id());
    agents.dedup();
    let one = |agent: Agent| match (agent, roots.root(agent)) {
        (_, None) => vec![],
        (Agent::ClaudeCode, Some(root)) => claude::recent(root, limit),
        (Agent::Codex, Some(root)) => codex::recent(root, limit),
        (Agent::Hermes, Some(root)) => hermes::recent(root, limit),
    };
    // Each agent's store is read on its own thread: one slow store doesn't add to the others.
    let mut all: Vec<ThreadSummary> = if agents.len() == 1 {
        one(agents[0])
    } else {
        std::thread::scope(|s| {
            let handles: Vec<_> = agents.iter().map(|a| s.spawn(move || one(*a))).collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        })
    };
    all.sort_by(|a, b| {
        b.last_active_ms
            .cmp(&a.last_active_ms)
            .then_with(|| a.id.cmp(&b.id))
    });
    all.truncate(limit);
    all
}

/// How long [`recent_cached`] keeps a listing.
pub const RECENT_CACHE_FOR: std::time::Duration = std::time::Duration::from_secs(30);

type RecentKey = (Roots, Vec<Agent>, usize);
type RecentCache = std::sync::Mutex<Vec<(RecentKey, std::time::Instant, Vec<ThreadSummary>)>>;

fn recent_cache() -> &'static RecentCache {
    static C: std::sync::OnceLock<RecentCache> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// [`recent`], kept for [`RECENT_CACHE_FOR`] per roots, agents and limit, so
/// a screen that asks again (the setup's last screen, then Home) answers at
/// once. Only titles, folders and times are kept, in this process's memory.
pub fn recent_cached(roots: &Roots, opts: &ListOptions) -> Vec<ThreadSummary> {
    let mut agents = opts.agents.clone();
    agents.sort_by_key(|a| a.id());
    agents.dedup();
    let key: RecentKey = (roots.clone(), agents, opts.limit.clamp(1, RECENT_MAX));
    if let Ok(c) = recent_cache().lock()
        && let Some((_, at, v)) = c.iter().find(|(k, _, _)| *k == key)
        && at.elapsed() < RECENT_CACHE_FOR
    {
        return v.clone();
    }
    let found = recent(roots, opts);
    if let Ok(mut c) = recent_cache().lock() {
        c.retain(|(k, at, _)| *k != key && at.elapsed() < RECENT_CACHE_FOR);
        c.push((key, std::time::Instant::now(), found.clone()));
    }
    found
}

/// One chat's messages, the newest `max_messages` of them (default
/// [`HISTORY_DEFAULT`]).
pub fn history(
    roots: &Roots,
    agent: Agent,
    id: &str,
    max_messages: Option<usize>,
) -> Result<ThreadHistory> {
    if !valid_id(id) {
        return Err(Error::Invalid(format!("{id:?} is not a session id")));
    }
    let not_found = || Error::NotFound {
        agent: agent.id(),
        id: id.to_string(),
    };
    let root = roots.root(agent).ok_or_else(not_found)?;
    let max = max_messages.unwrap_or(HISTORY_DEFAULT).clamp(1, 5000);
    // Agent replies in a row are merged into one afterwards, so more than
    // `max` are read; only the newest part of a chat is ever read.
    let cap = max.saturating_mul(4).saturating_add(64);
    let (thread, messages, unread) = match agent {
        Agent::ClaudeCode => claude::history(root, id, cap)?,
        Agent::Codex => codex::history(root, id, cap)?,
        Agent::Hermes => hermes::history(root, id, cap)?,
    }
    .ok_or_else(not_found)?;
    let mut messages = text::merge_agent_runs(messages);
    // A lower bound when older messages were left unread: at least one more.
    let earlier_omitted = messages.len().saturating_sub(max).max(usize::from(unread));
    messages.drain(..messages.len().saturating_sub(max));
    for m in &mut messages {
        let (t, cut) = text::cut_bytes(&m.text, MESSAGE_MAX_BYTES);
        m.cut |= cut;
        m.text = t;
    }
    Ok(ThreadHistory {
        thread,
        messages,
        earlier_omitted,
    })
}

/// A session id: 1 to 128 letters, digits, `-`, `_` or `.` (never a path).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id != "."
        && id != ".."
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// An RFC 3339 time as Unix ms.
pub(crate) fn rfc3339_ms(s: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .and_then(|t| u64::try_from(t.timestamp_millis()).ok())
}

/// A file's modification time as Unix ms.
pub(crate) fn mtime_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_never_name_a_path() {
        assert!(valid_id("0b1e5c2a-1111-4222-8333-944445555666"));
        assert!(valid_id("20261104_101112_ab12cd"));
        for bad in [
            "",
            ".",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "x y",
            &"a".repeat(129),
        ] {
            assert!(!valid_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn agent_ids_round_trip() {
        for a in Agent::ALL {
            assert_eq!(Agent::parse(a.id()), Some(a));
            let j = serde_json::to_string(&a).unwrap();
            assert_eq!(serde_json::from_str::<Agent>(&j).unwrap(), a);
        }
        assert_eq!(Agent::parse("codex"), Some(Agent::Codex));
        assert_eq!(
            Agent::Codex.resume_command("x"),
            vec!["codex".to_string(), "resume".into(), "x".into()]
        );
    }

    /// A chat of hundreds of megabytes opens by reading the end of its file
    /// (CUA-3548: the whole file was read and every message kept).
    #[test]
    fn a_huge_chat_is_read_from_its_end_only() {
        use std::io::Write;
        let home = tempfile::tempdir().unwrap();
        let id = "0b1e5c2a-1111-4222-8333-944445555666";
        let path = home
            .path()
            .join(format!(".claude/projects/-Users-sam-kettle/{id}.jsonl"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        let pad = "x".repeat(200_000);
        for n in 0..400 {
            writeln!(f, r#"{{"type":"user","uuid":"u{n}","cwd":"/k","timestamp":"2026-11-04T10:00:00Z","message":{{"role":"user","content":"q{n}"}}}}"#).unwrap();
            writeln!(f, r#"{{"type":"user","uuid":"t{n}","message":{{"role":"user","content":[{{"type":"tool_result","content":"{pad}"}}]}}}}"#).unwrap();
            writeln!(f, r#"{{"type":"assistant","uuid":"a{n}","message":{{"role":"assistant","content":[{{"type":"text","text":"a{n}"}}]}}}}"#).unwrap();
        }
        drop(f);
        assert!(std::fs::metadata(&path).unwrap().len() > 70 * 1024 * 1024);
        let roots = Roots::under_home(home.path());
        lines::REV_BYTES_READ.with(|c| c.set(0));
        let h = history(&roots, Agent::ClaudeCode, id, Some(30)).unwrap();
        assert_eq!(h.messages.len(), 30);
        assert_eq!(h.messages.last().unwrap().text, "a399");
        assert!(h.earlier_omitted > 0);
        // The listing's head and tail edges are on top of this; the reader itself stays in its budget.
        assert!(
            lines::REV_BYTES_READ.with(|c| c.get()) <= lines::HISTORY_BUDGET.bytes + 512 * 1024
        );
    }
}
