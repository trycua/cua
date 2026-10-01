// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The notifications feed: what the apps show as system notifications.
//!
//! The daemon appends here when a persistent agent's turn ends ("Your
//! research is ready"), when an agent calls `notify_user`, and when a
//! routine or a run fails. The apps poll `notifications_list`, post a system
//! notification for each entry they have not shown, and mark entries read.
//! The feed is a file, so nothing is lost while no app is open.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use cua_spaces::{Error, Result};

/// Entries kept (oldest dropped first).
pub const MAX_ENTRIES: usize = 500;
/// Longest title and body stored.
const MAX_TITLE: usize = 120;
const MAX_BODY: usize = 1000;

/// One notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    /// Unix ms.
    pub at_ms: u64,
    /// The persistent agent it is about, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// `turn_ended`, `message` (the agent called `notify_user`), `error`,
    /// `approval` (something waits for the user).
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
    #[serde(default)]
    pub read: bool,
}

/// The feed file (`<home>/persistent/notifications.json`).
#[derive(Clone, Debug)]
pub struct Feed {
    path: PathBuf,
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

impl Feed {
    pub fn new(dir: &Path) -> Feed {
        Feed {
            path: dir.join("notifications.json"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn with<R>(&self, f: impl FnOnce(&mut Vec<Notification>) -> R) -> Result<R> {
        super::locked_json(&self.path, f)
    }

    /// Adds one entry and returns it.
    pub fn post(
        &self,
        agent: Option<&str>,
        kind: &str,
        title: &str,
        body: &str,
        run_id: Option<&str>,
        space: Option<&str>,
    ) -> Result<Notification> {
        let title = clip(title, MAX_TITLE);
        if title.is_empty() {
            return Err(Error::invalid("a notification needs a title"));
        }
        let n = Notification {
            id: cua_volume::new_id(),
            at_ms: cua_volume::now_ms(),
            agent: agent.map(str::to_string),
            kind: kind.into(),
            title,
            body: clip(body, MAX_BODY),
            run_id: run_id.map(str::to_string),
            space: space.map(str::to_string),
            read: false,
        };
        let out = n.clone();
        self.with(move |all| {
            all.push(n);
            let extra = all.len().saturating_sub(MAX_ENTRIES);
            all.drain(..extra);
        })?;
        Ok(out)
    }

    /// Entries newest first; `unread_only` drops read ones; `since_ms`
    /// keeps only newer ones.
    pub fn list(&self, unread_only: bool, since_ms: Option<u64>) -> Result<Vec<Notification>> {
        let all: Vec<Notification> = super::read_json(&self.path)?;
        Ok(all
            .into_iter()
            .rev()
            .filter(|n| !unread_only || !n.read)
            .filter(|n| since_ms.is_none_or(|s| n.at_ms > s))
            .collect())
    }

    /// Marks `ids` read (every entry when `ids` is empty). Returns how many
    /// changed.
    pub fn ack(&self, ids: &[String]) -> Result<usize> {
        self.with(|all| {
            let mut n = 0;
            for e in all.iter_mut() {
                if !e.read && (ids.is_empty() || ids.contains(&e.id)) {
                    e.read = true;
                    n += 1;
                }
            }
            n
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_list_ack_and_cap() {
        let dir = tempfile::tempdir().unwrap();
        let f = Feed::new(dir.path());
        assert!(f.post(None, "message", "  ", "", None, None).is_err());
        let a = f
            .post(
                Some("ada"),
                "turn_ended",
                "ada",
                "Your research is ready.",
                Some("run-1"),
                None,
            )
            .unwrap();
        let b = f
            .post(
                Some("ada"),
                "message",
                "Heads up",
                &"x".repeat(5000),
                None,
                None,
            )
            .unwrap();
        assert_eq!(b.body.chars().count(), MAX_BODY);
        let all = f.list(false, None).unwrap();
        assert_eq!(all[0].id, b.id, "newest first");
        assert_eq!(f.ack(std::slice::from_ref(&a.id)).unwrap(), 1);
        assert_eq!(f.list(true, None).unwrap().len(), 1);
        assert_eq!(f.ack(&[]).unwrap(), 1);
        assert!(f.list(true, None).unwrap().is_empty());
        for i in 0..MAX_ENTRIES + 5 {
            f.post(None, "message", &format!("n{i}"), "", None, None)
                .unwrap();
        }
        assert_eq!(f.list(false, None).unwrap().len(), MAX_ENTRIES);
    }
}
