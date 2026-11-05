//! Hermes: `<home>/state.db`, SQLite, opened read-only.
//!
//! The tables used: `sessions` (`id`, and when present `title`, `cwd`,
//! `source`, `parent_session_id`, `hidden`, `archived`, `started_at`,
//! `last_activity_at`, `ended_at`, times in Unix seconds) and `messages`
//! (`session_id`, `role`, `content`, `timestamp`, and `active` when
//! present). Columns differ between versions, so the query uses only those
//! the table has. Scheduled (`cron`) and subagent sessions, child sessions
//! and hidden or archived ones are not listed.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;

use crate::text::{injected, title};
use crate::{Agent, Error, Message, Result, Role, ThreadSummary};

fn open(root: &Path) -> Option<Connection> {
    let path = root.join("state.db");
    if !path.is_file() {
        return None;
    }
    let c = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let _ = c.busy_timeout(std::time::Duration::from_millis(500));
    Some(c)
}

fn columns(c: &Connection, table: &str) -> Vec<String> {
    let Ok(mut s) = c.prepare(&format!("PRAGMA table_info({table})")) else {
        return vec![];
    };
    s.query_map([], |r| r.get::<_, String>(1))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// The session query for `cols`: (select list, where clause, order).
fn session_sql(cols: &[String]) -> Option<(String, String, String)> {
    let has = |c: &str| cols.iter().any(|x| x == c);
    if !has("id") {
        return None;
    }
    let col = |c: &str| {
        if has(c) {
            c.to_string()
        } else {
            "NULL".to_string()
        }
    };
    let times: Vec<&str> = ["last_activity_at", "ended_at", "started_at"]
        .into_iter()
        .filter(|c| has(c))
        .collect();
    let when = match times.len() {
        0 => "0".to_string(),
        1 => times[0].to_string(),
        _ => format!("COALESCE({})", times.join(", ")),
    };
    let select = format!("id, {}, {}, {when}", col("title"), col("cwd"));
    let mut wh = vec!["1".to_string()];
    if has("source") {
        wh.push("COALESCE(source, '') NOT IN ('cron', 'subagent')".into());
    }
    if has("parent_session_id") {
        wh.push("(parent_session_id IS NULL OR parent_session_id = '')".into());
    }
    for flag in ["hidden", "archived"] {
        if has(flag) {
            wh.push(format!("COALESCE({flag}, 0) = 0"));
        }
    }
    Some((select, wh.join(" AND "), format!("{when} DESC")))
}

struct Row {
    id: String,
    title: Option<String>,
    cwd: Option<String>,
    when_s: f64,
}

fn rows(c: &Connection, extra: &str, limit: usize, id: Option<&str>) -> Vec<Row> {
    let Some((select, wh, order)) = session_sql(&columns(c, "sessions")) else {
        return vec![];
    };
    let sql = format!("SELECT {select} FROM sessions WHERE {wh}{extra} ORDER BY {order} LIMIT ?1");
    let Ok(mut s) = c.prepare(&sql) else {
        return vec![];
    };
    let map = |r: &rusqlite::Row| {
        Ok(Row {
            id: r.get(0)?,
            title: r.get::<_, Option<String>>(1).ok().flatten(),
            cwd: r.get::<_, Option<String>>(2).ok().flatten(),
            when_s: r.get::<_, Option<f64>>(3).ok().flatten().unwrap_or(0.0),
        })
    };
    let got = match id {
        Some(id) => s.query_map(params![limit as i64, id], map),
        None => s.query_map(params![limit as i64], map),
    };
    got.map(|rows| rows.flatten().collect()).unwrap_or_default()
}

/// The text of a message's `content`: plain, or a JSON list of parts.
fn content_text(raw: &str) -> String {
    let t = raw.trim_start();
    if t.starts_with('[')
        && let Ok(Value::Array(parts)) = serde_json::from_str::<Value>(t)
    {
        return parts
            .iter()
            .filter(|p| {
                p.get("type")
                    .and_then(Value::as_str)
                    .is_none_or(|k| k == "text")
            })
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n\n");
    }
    raw.to_string()
}

fn first_prompt(c: &Connection, session: &str) -> Option<String> {
    let mut s = c
        .prepare("SELECT content FROM messages WHERE session_id = ?1 AND role = 'user' ORDER BY id LIMIT 20")
        .ok()?;
    let found = s
        .query_map(params![session], |r| r.get::<_, Option<String>>(0))
        .ok()?
        .flatten()
        .flatten()
        .map(|c| content_text(&c))
        .find(|t| !injected(t));
    found.and_then(|t| title(&t))
}

fn summary(c: &Connection, r: Row) -> Option<ThreadSummary> {
    if !crate::valid_id(&r.id) {
        return None;
    }
    // A session nobody typed into is not a chat.
    let prompt = first_prompt(c, &r.id)?;
    Some(ThreadSummary {
        agent: Agent::Hermes,
        resume: Agent::Hermes.resume_command(&r.id),
        title: r.title.as_deref().and_then(title).unwrap_or(prompt),
        cwd: r.cwd.filter(|s| !s.is_empty()),
        last_active_ms: (r.when_s.max(0.0) * 1000.0) as u64,
        id: r.id,
    })
}

pub(crate) fn recent(root: &Path, limit: usize) -> Vec<ThreadSummary> {
    let Some(c) = open(root) else { return vec![] };
    rows(&c, "", limit * 4, None)
        .into_iter()
        .filter_map(|r| summary(&c, r))
        .take(limit)
        .collect()
}

pub(crate) fn history(
    root: &Path,
    id: &str,
    cap: usize,
) -> Result<Option<(ThreadSummary, Vec<Message>, bool)>> {
    let Some(c) = open(root) else { return Ok(None) };
    let Some(row) = rows(&c, " AND id = ?2", 1, Some(id)).into_iter().next() else {
        return Ok(None);
    };
    let Some(thread) = summary(&c, row) else {
        return Ok(None);
    };
    let active = if columns(&c, "messages").iter().any(|c| c == "active") {
        " AND COALESCE(active, 1) = 1"
    } else {
        ""
    };
    let sql = format!(
        "SELECT id, role, content, timestamp FROM messages \
         WHERE session_id = ?1 AND role IN ('user', 'assistant'){active} ORDER BY id DESC LIMIT ?2"
    );
    let sqlite = |e: rusqlite::Error| Error::Sqlite(e.to_string());
    let mut s = c.prepare(&sql).map_err(sqlite)?;
    let messages = s
        .query_map(params![id, cap as i64 + 1], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<f64>>(3).ok().flatten(),
            ))
        })
        .map_err(sqlite)?
        .flatten()
        .filter_map(|(key, role, content, ts)| {
            let role = if role == "user" {
                Role::Person
            } else {
                Role::Agent
            };
            let text = content_text(&content?);
            if text.trim().is_empty() || (role == Role::Person && injected(&text)) {
                return None;
            }
            Some(Message {
                key: key.to_string(),
                role,
                text,
                ts_ms: ts.map(|s| (s.max(0.0) * 1000.0) as u64),
                cut: false,
            })
        })
        .collect::<Vec<_>>();
    let more = messages.len() > cap;
    let mut messages = messages;
    messages.truncate(cap);
    messages.reverse();
    Ok(Some((thread, messages, more)))
}
