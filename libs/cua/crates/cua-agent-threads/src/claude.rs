//! Claude Code: `<config>/projects/<folder slug>/<session id>.jsonl`, one
//! JSON record per line.
//!
//! The records used: `user` and `assistant` (`message.content` a string or
//! content blocks; `cwd`, `timestamp`, `uuid`, `isMeta`, `isSidechain`),
//! and the titles `custom-title` (`customTitle`, set by `/rename`),
//! `ai-title` (`aiTitle`) and `summary` (`summary`, older versions). A
//! subagent's own chats (`<session>/subagents/*.jsonl`) are one level
//! deeper and are not listed.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::lines::{self, str_at};
use crate::text::{injected, title};
use crate::{Agent, Message, Result, Role, ThreadSummary, mtime_ms, rfc3339_ms};

/// Files looked at per chat listed: most hold a real chat, some (a run
/// that never got a prompt) do not.
const SCAN_FACTOR: usize = 8;

pub(crate) fn recent(root: &Path, limit: usize) -> Vec<ThreadSummary> {
    let mut files = sessions(root);
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    files
        .into_iter()
        .take(limit * SCAN_FACTOR)
        .filter_map(|(path, mtime)| summary(&path, mtime))
        .take(limit)
        .collect()
}

/// Every `projects/*/*.jsonl` with its modification time.
fn sessions(root: &Path) -> Vec<(PathBuf, u64)> {
    let Ok(projects) = std::fs::read_dir(root.join("projects")) else {
        return vec![];
    };
    let mut out = vec![];
    for project in projects.flatten() {
        let Ok(files) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for f in files.flatten() {
            let path = f.path();
            if path.extension().is_some_and(|e| e == "jsonl")
                && let Ok(meta) = f.metadata()
                && meta.is_file()
                && id_of(&path).is_some()
            {
                out.push((path, mtime_ms(&meta)));
            }
        }
    }
    out
}

fn id_of(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    crate::valid_id(stem).then(|| stem.to_string())
}

#[derive(Default)]
struct Titles {
    custom: Option<String>,
    ai: Option<String>,
    summary: Option<String>,
}

impl Titles {
    fn read(&mut self, v: &Value) {
        match v.get("type").and_then(Value::as_str) {
            Some("custom-title") => {
                self.custom = str_at(v, &["customTitle"])
                    .and_then(title)
                    .or(self.custom.take())
            }
            Some("ai-title") => {
                self.ai = str_at(v, &["aiTitle"]).and_then(title).or(self.ai.take())
            }
            Some("summary") => {
                self.summary = str_at(v, &["summary"])
                    .and_then(title)
                    .or(self.summary.take())
            }
            _ => {}
        }
    }

    fn best(self) -> Option<String> {
        self.custom.or(self.ai).or(self.summary)
    }
}

fn summary(path: &Path, mtime: u64) -> Option<ThreadSummary> {
    let id = id_of(path)?;
    let (head, tail) = lines::edges(path).ok()?;
    let mut titles = Titles::default();
    let mut cwd = None;
    let mut first_prompt = None;
    let mut last_ts = None::<u64>;
    for v in head.iter().chain(tail.iter()) {
        titles.read(v);
        if cwd.is_none() {
            cwd = str_at(v, &["cwd"])
                .filter(|s| !s.is_empty())
                .map(String::from);
        }
        if first_prompt.is_none()
            && let Some(m) = message(v)
            && m.role == Role::Person
        {
            first_prompt = title(&m.text);
        }
        if let Some(t) = str_at(v, &["timestamp"]).and_then(rfc3339_ms) {
            last_ts = Some(last_ts.map_or(t, |l: u64| l.max(t)));
        }
    }
    // A chat with nothing the person said is a run that never started.
    let prompt = first_prompt?;
    Some(ThreadSummary {
        agent: Agent::ClaudeCode,
        resume: Agent::ClaudeCode.resume_command(&id),
        id,
        title: titles.best().unwrap_or(prompt),
        cwd,
        // The records' own times; a copied file's mtime says nothing.
        last_active_ms: last_ts.unwrap_or(mtime),
    })
}

/// The message a `user` or `assistant` record carries, if it is one the
/// person typed or a reply's text.
fn message(v: &Value) -> Option<Message> {
    let role = match v.get("type").and_then(Value::as_str)? {
        "user" => Role::Person,
        "assistant" => Role::Agent,
        _ => return None,
    };
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    if flag("isSidechain") || flag("isMeta") || flag("isCompactSummary") {
        return None;
    }
    let content = v.get("message")?.get("content")?;
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => return None,
    };
    if text.trim().is_empty() || (role == Role::Person && injected(&text)) {
        return None;
    }
    Some(Message {
        key: str_at(v, &["uuid"]).unwrap_or_default().to_string(),
        role,
        text,
        ts_ms: str_at(v, &["timestamp"]).and_then(rfc3339_ms),
        cut: false,
    })
}

/// A chat's newest messages: the file is read from its end, within
/// [`lines::HISTORY_BUDGET`], until `cap` messages are in hand, so a chat of
/// any size opens at once. `more` is whether older ones were left unread.
pub(crate) fn history(
    root: &Path,
    id: &str,
    cap: usize,
) -> Result<Option<(ThreadSummary, Vec<Message>, bool)>> {
    let Some(path) = find(root, id) else {
        return Ok(None);
    };
    let mtime = std::fs::metadata(&path).map(|m| mtime_ms(&m)).unwrap_or(0);
    let Some(thread) = summary(&path, mtime) else {
        return Ok(None);
    };
    let mut out = vec![];
    let whole = lines::rev_each(&path, lines::HISTORY_BUDGET, |v| {
        if let Some(mut m) = message(&v) {
            if m.key.is_empty() {
                m.key = format!("rev-{}", out.len());
            }
            out.push(m);
        }
        out.len() < cap
    })
    .map_err(|source| crate::Error::Io {
        path: path.display().to_string(),
        source,
    })?;
    out.reverse();
    Ok(Some((thread, out, !whole)))
}

fn find(root: &Path, id: &str) -> Option<PathBuf> {
    let name = format!("{id}.jsonl");
    std::fs::read_dir(root.join("projects"))
        .ok()?
        .flatten()
        .map(|p| p.path().join(&name))
        .find(|p| p.is_file())
}
