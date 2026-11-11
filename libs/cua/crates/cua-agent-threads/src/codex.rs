//! Codex: `<home>/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`, one JSON
//! record per line, and `<home>/session_index.jsonl` (`id`, `thread_name`,
//! `updated_at`) for the names chats were given.
//!
//! The records used: `session_meta` (`payload.id`, `payload.cwd`,
//! `payload.source`), `response_item` with `payload.type` `message`
//! (role `user` with `input_text`, role `assistant` with `output_text`; the
//! `developer` role is Codex's own instructions), and, from older versions,
//! `event_msg` `user_message`/`agent_message` (`payload.message`). As in
//! `codex resume`, only interactive chats are listed: `codex exec` runs,
//! MCP-started and subagent sessions are not.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::lines::{self, str_at};
use crate::text::{injected, title};
use crate::{Agent, Message, Result, Role, ThreadSummary, mtime_ms, rfc3339_ms};

const SCAN_FACTOR: usize = 8;
/// The index is read whole up to this size, otherwise its tail only.
const INDEX_MAX: u64 = 4 * 1024 * 1024;

pub(crate) fn recent(root: &Path, limit: usize) -> Vec<ThreadSummary> {
    let mut files = vec![];
    newest(&root.join("sessions"), limit * SCAN_FACTOR, &mut files);
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    let names = index(root);
    files
        .into_iter()
        .take(limit * SCAN_FACTOR)
        .filter_map(|(path, mtime)| summary(&path, mtime, &names))
        .take(limit)
        .collect()
}

/// Day folders read past the one where enough chats were found: a chat
/// started earlier and taken up again since is written to its first day's
/// folder, so a week back still finds it.
const LOOKBACK_DAYS: usize = 7;

/// A `rollout-*.jsonl` name (Codex's archived `.jsonl.zst` and backups are not chats).
fn is_rollout(name: &std::ffi::OsStr) -> bool {
    name.to_str()
        .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
}

/// `rollout-*.jsonl` files up to four folders down.
fn walk(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, u64)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() && depth < 4 {
            walk(&e.path(), depth + 1, out);
        } else if !kind.is_dir()
            && is_rollout(&e.file_name())
            && let Ok(meta) = e.metadata()
            && meta.is_file()
        {
            out.push((e.path(), mtime_ms(&meta)));
        }
    }
}

/// The newest rollouts for a listing: Codex's `YYYY/MM/DD` folders newest
/// first, stopping [`LOOKBACK_DAYS`] folders after `want` files were found,
/// so a store of years of chats is not read whole. Within reach, the order
/// is still each file's last change.
fn newest(dir: &Path, want: usize, out: &mut Vec<(PathBuf, u64)>) {
    let mut extra = None::<usize>;
    newest_in(dir, 0, want, &mut extra, out);
}

/// Returns false once the listing has read far enough.
fn newest_in(
    dir: &Path,
    depth: usize,
    want: usize,
    extra: &mut Option<usize>,
    out: &mut Vec<(PathBuf, u64)>,
) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return true;
    };
    let (mut dirs, mut files) = (vec![], false);
    for e in entries.flatten() {
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            if depth < 4 {
                dirs.push(e.path());
            }
        } else if is_rollout(&e.file_name())
            && let Ok(meta) = e.metadata()
            && meta.is_file()
        {
            out.push((e.path(), mtime_ms(&meta)));
            files = true;
        }
    }
    if files {
        if let Some(n) = extra.as_mut() {
            *n += 1;
        } else if out.len() >= want {
            *extra = Some(0);
        }
        if extra.is_some_and(|n| n >= LOOKBACK_DAYS) {
            return false;
        }
    }
    // `2026` > `2025`, `11` > `10`: the names sort as the dates do.
    dirs.sort_unstable_by(|a, b| b.file_name().cmp(&a.file_name()));
    dirs.iter()
        .all(|d| newest_in(d, depth + 1, want, extra, out))
}

/// The chats' names from `session_index.jsonl`, the latest per id.
fn index(root: &Path) -> HashMap<String, String> {
    let path = root.join("session_index.jsonl");
    let mut out = HashMap::new();
    let Ok(meta) = std::fs::metadata(&path) else {
        return out;
    };
    let mut add = |v: &Value| {
        if let (Some(id), Some(name)) = (
            str_at(v, &["id"]),
            str_at(v, &["thread_name"]).and_then(title),
        ) {
            out.insert(id.to_string(), name);
        }
    };
    if meta.len() <= INDEX_MAX {
        let _ = lines::each(&path, |v| add(&v));
    } else if let Ok((_, tail)) = lines::edges(&path) {
        tail.iter().for_each(add);
    }
    out
}

/// A session's sources that are not someone's own chat.
fn interactive(source: Option<&Value>) -> bool {
    match source {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => {
            matches!(s.as_str(), "cli" | "vscode" | "tui" | "app" | "desktop")
        }
        // `{"subagent": ...}` and other structured sources are spawned.
        Some(_) => false,
    }
}

struct Meta {
    id: String,
    cwd: Option<String>,
}

fn meta(v: &Value) -> Option<Meta> {
    if v.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let p = v.get("payload")?;
    if !interactive(p.get("source")) {
        return None;
    }
    let id = str_at(p, &["id"]).or_else(|| str_at(p, &["session_id"]))?;
    crate::valid_id(id).then(|| Meta {
        id: id.to_string(),
        cwd: str_at(p, &["cwd"])
            .filter(|s| !s.is_empty())
            .map(String::from),
    })
}

fn summary(path: &Path, mtime: u64, names: &HashMap<String, String>) -> Option<ThreadSummary> {
    let (head, tail) = lines::edges(path).ok()?;
    let m = head.iter().take(8).find_map(meta)?;
    let mut first_prompt = None;
    let mut last_ts = None::<u64>;
    for v in head.iter().chain(tail.iter()) {
        if first_prompt.is_none()
            && let Some(msg) = message(v).or_else(|| event_message(v))
            && msg.role == Role::Person
        {
            first_prompt = title(&msg.text);
        }
        if let Some(t) = str_at(v, &["timestamp"]).and_then(rfc3339_ms) {
            last_ts = Some(last_ts.map_or(t, |l: u64| l.max(t)));
        }
    }
    let prompt = first_prompt?;
    Some(ThreadSummary {
        agent: Agent::Codex,
        resume: Agent::Codex.resume_command(&m.id),
        title: names.get(&m.id).cloned().unwrap_or(prompt),
        id: m.id,
        cwd: m.cwd,
        last_active_ms: last_ts.unwrap_or(mtime),
    })
}

fn texts(content: &Value, kind: &str) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some(kind))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    }
}

/// A `response_item` message the person typed or the agent answered.
fn message(v: &Value) -> Option<Message> {
    if v.get("type").and_then(Value::as_str) != Some("response_item") {
        return None;
    }
    let p = v.get("payload")?;
    if p.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let (role, kind) = match p.get("role").and_then(Value::as_str)? {
        "user" => (Role::Person, "input_text"),
        "assistant" => (Role::Agent, "output_text"),
        _ => return None,
    };
    build(v, p, role, texts(p.get("content")?, kind))
}

/// Older versions: `event_msg` `user_message` / `agent_message`.
fn event_message(v: &Value) -> Option<Message> {
    if v.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let p = v.get("payload")?;
    let role = match p.get("type").and_then(Value::as_str)? {
        "user_message" => Role::Person,
        "agent_message" => Role::Agent,
        _ => return None,
    };
    build(v, p, role, str_at(p, &["message"])?.to_string())
}

fn build(v: &Value, p: &Value, role: Role, text: String) -> Option<Message> {
    if text.trim().is_empty() || (role == Role::Person && injected(&text)) {
        return None;
    }
    Some(Message {
        key: str_at(p, &["id"]).unwrap_or_default().to_string(),
        role,
        text,
        ts_ms: str_at(v, &["timestamp"]).and_then(rfc3339_ms),
        cut: false,
    })
}

/// A chat's newest messages, read from the end of the file within
/// [`lines::HISTORY_BUDGET`]; see [`crate::claude::history`].
pub(crate) fn history(
    root: &Path,
    id: &str,
    cap: usize,
) -> Result<Option<(ThreadSummary, Vec<Message>, bool)>> {
    let Some(path) = find(root, id) else {
        return Ok(None);
    };
    let mtime = std::fs::metadata(&path).map(|m| mtime_ms(&m)).unwrap_or(0);
    let Some(thread) = summary(&path, mtime, &index(root)) else {
        return Ok(None);
    };
    if thread.id != id {
        return Ok(None);
    }
    let (mut items, mut events) = (vec![], vec![]);
    let whole = lines::rev_each(&path, lines::HISTORY_BUDGET, |v| {
        if let Some(m) = message(&v) {
            items.push(m);
        } else if let Some(m) = event_message(&v) {
            events.push(m);
        }
        items.len() < cap && events.len() < cap
    })
    .map_err(|source| crate::Error::Io {
        path: path.display().to_string(),
        source,
    })?;
    // Newer rollouts write each message both ways: the response items win.
    let mut out = if items.is_empty() { events } else { items };
    out.reverse();
    for (i, m) in out.iter_mut().enumerate() {
        if m.key.is_empty() {
            m.key = format!("item-{i}");
        }
    }
    Ok(Some((thread, out, !whole)))
}

/// The rollout whose name ends in `-<id>.jsonl`.
fn find(root: &Path, id: &str) -> Option<PathBuf> {
    let mut files = vec![];
    walk(&root.join("sessions"), 0, &mut files);
    let suffix = format!("-{id}.jsonl");
    files.into_iter().map(|(p, _)| p).find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(&suffix))
    })
}
