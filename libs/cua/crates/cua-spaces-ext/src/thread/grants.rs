// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! "Allow for this Space": a person's standing answer to one kind of
//! request from one agent.
//!
//! A person answers an agent's approval with `scope: "space"` and the
//! daemon remembers the request's action (the agent's short label for what
//! it asks, for example "install a package"), for that agent, in that
//! Space. The next approval with the same action from the same agent is
//! answered by the daemon the moment it is posted: an answer entry written
//! by `system:grant`, `approve: true`, `scope: "space"`, naming the answer
//! it follows. The thread shows it like any answer, and the person can
//! take the grant back (`thread_grant_revoke`), after which the agent asks
//! again.
//!
//! Only an agent's own plain request can be granted. Never:
//!
//! - an approval the host asks about (a push to a repository carries a
//!   card bound to one commit: each push is asked on its own);
//! - an `ability:` request (what a home Space may reach is the app's
//!   decision, remembered by the app);
//! - a join request;
//! - a decline.
//!
//! [`scope_choices`] says which a request offers, as data for the phone to
//! draw; the daemon refuses a scope the request does not offer.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::{AuthorKind, ThreadEntry, ThreadStore};
use cua_spaces::{Error, Result};

/// The file under the thread's folder.
pub const FILE: &str = "grants.json";
/// Most grants one Space keeps.
pub const MAX: usize = 100;
/// Longest action kept, in characters.
const ACTION_MAX: usize = 200;

/// One standing answer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    /// `agent` and the normalized action ([`key`]).
    pub key: String,
    /// The agent's key (its name on this machine).
    pub agent: String,
    /// The action as the agent wrote it.
    pub action: String,
    /// The approval whose answer made the grant.
    pub approval: u64,
    /// Who allowed it (the author id) and their name.
    pub by: String,
    #[serde(default)]
    pub by_name: String,
    pub ts_ms: u64,
    /// The enrolled device that gave the answer (the relay vouched for it), and
    /// whether it is a phone: "allowed from an iPhone".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub from_phone: bool,
}

/// The action as compared: lowercase, one line, spaces collapsed.
pub fn normalize(action: &str) -> String {
    let flat = action
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    flat.chars().take(ACTION_MAX).collect()
}

/// A grant's key.
pub fn key(agent: &str, action: &str) -> String {
    format!("{agent}\u{0}{}", normalize(action))
}

/// What a request offers an answerer: `["once", "space"]` for an agent's
/// own plain request, `["once"]` for everything else.
pub fn scope_choices(e: &ThreadEntry) -> &'static [&'static str] {
    let Some(a) = &e.approval else {
        return &["once"];
    };
    let host = e.author.kind != AuthorKind::Agent;
    if host
        || a.card.is_some()
        || a.action == crate::persistent::host_tools::REPO_PUSH
        || a.action.starts_with("ability:")
        || e.join_request.is_some()
    {
        &["once"]
    } else {
        &["once", "space"]
    }
}

/// What kind of request an approval is, as a word the phone can draw a
/// grant's meaning from. Decided from what the host knows, never from the
/// agent's words: `push` (the host's push card), `join` (an agent asking
/// to join), `ability` (a home Space asking for reach) or `request` (an
/// agent's own request).
pub fn kind(e: &ThreadEntry) -> &'static str {
    if e.join_request.is_some() {
        return "join";
    }
    match &e.approval {
        Some(a) if a.card.is_some() || a.action == crate::persistent::host_tools::REPO_PUSH => {
            "push"
        }
        Some(a) if a.action.starts_with("ability:") => "ability",
        _ => "request",
    }
}

fn lock() -> &'static Mutex<()> {
    static L: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(Default::default)
}

fn path(store: &ThreadStore) -> PathBuf {
    store.dir().join(FILE)
}

fn load(store: &ThreadStore) -> Result<Vec<Grant>> {
    match std::fs::read(path(store)) {
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(Error::from(e)),
    }
}

fn save(store: &ThreadStore, all: &[Grant]) -> Result<()> {
    std::fs::create_dir_all(store.dir())?;
    cua_home::write_private(&path(store), &serde_json::to_vec_pretty(all)?).map_err(Error::from)
}

/// The Space's grants, oldest first.
pub fn list(store: &ThreadStore) -> Result<Vec<Grant>> {
    let _l = lock().lock().expect("grants lock");
    load(store)
}

/// How long a standing answer holds: a day. After that the same request asks again,
/// so an answer given from a phone that is later lost does not stand for good.
pub const TTL_MS: u64 = 24 * 3600 * 1000;

/// The grant covering `action` from `agent`, if any and not older than [`TTL_MS`].
pub fn find(store: &ThreadStore, agent: &str, action: &str) -> Result<Option<Grant>> {
    find_at(store, agent, action, cua_volume::now_ms())
}

/// [`find`] at a given time (milliseconds since the epoch).
pub fn find_at(
    store: &ThreadStore,
    agent: &str,
    action: &str,
    now_ms: u64,
) -> Result<Option<Grant>> {
    let k = key(agent, action);
    Ok(list(store)?
        .into_iter()
        .find(|g| g.key == k && now_ms.saturating_sub(g.ts_ms) < TTL_MS))
}

/// Remembers a grant (the same request again keeps the first).
pub fn add(store: &ThreadStore, g: Grant) -> Result<()> {
    let _l = lock().lock().expect("grants lock");
    let mut all = load(store)?;
    // Expired answers go first: one that ended a day ago must not stop its successor
    // from being kept.
    let now = cua_volume::now_ms();
    all.retain(|x| now.saturating_sub(x.ts_ms) < TTL_MS);
    if all.iter().any(|x| x.key == g.key) {
        return Ok(());
    }
    if all.len() >= MAX {
        return Err(Error::invalid(format!(
            "this Space already holds {MAX} standing answers: take some back first"
        )));
    }
    all.push(g);
    save(store, &all)
}

/// Takes back the grant with `key`, or every grant of `agent` for
/// `action`. Returns how many went.
pub fn revoke(store: &ThreadStore, agent: &str, action: &str) -> Result<usize> {
    let _l = lock().lock().expect("grants lock");
    let mut all = load(store)?;
    let k = key(agent, action);
    let before = all.len();
    all.retain(|g| g.key != k);
    let gone = before - all.len();
    if gone > 0 {
        save(store, &all)?;
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::{Approval, Author, Draft, EntryKind};

    fn store() -> (tempfile::TempDir, ThreadStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = ThreadStore::open(dir.path(), "local:grants");
        (dir, s)
    }

    fn approval(author: Author, action: &str, card: Option<serde_json::Value>) -> ThreadEntry {
        let (_d, s) = store();
        let mut d = Draft::new(author, EntryKind::Approval, "asks");
        d.approval = Some(Approval {
            action: action.into(),
            details: String::new(),
            state: "open".into(),
            card,
        });
        s.append(d).unwrap()
    }

    #[test]
    fn actions_compare_without_case_or_spacing() {
        assert_eq!(normalize("  Install   a PACKAGE "), "install a package");
        assert_eq!(
            key("claude", "Install a package"),
            key("claude", "install  a package")
        );
        assert_ne!(
            key("claude", "install a package"),
            key("codex", "install a package")
        );
    }

    #[test]
    fn only_an_agents_own_plain_request_offers_a_standing_answer() {
        let plain = approval(Author::agent("claude"), "install a package", None);
        assert_eq!(scope_choices(&plain), ["once", "space"]);
        assert_eq!(kind(&plain), "request");
        let card = approval(
            Author::system("host"),
            crate::persistent::host_tools::REPO_PUSH,
            Some(serde_json::json!({"head": "x"})),
        );
        assert_eq!(scope_choices(&card), ["once"]);
        assert_eq!(kind(&card), "push");
        // An agent cannot dress its own request up as a push: it reads as
        // one, and offers once only.
        let faked = approval(Author::agent("claude"), "export:repo.push", None);
        assert_eq!(kind(&faked), "push");
        assert_eq!(scope_choices(&faked), ["once"]);
        let ability = approval(Author::agent("claude"), "ability:spaces.list", None);
        assert_eq!(scope_choices(&ability), ["once"]);
        assert_eq!(kind(&ability), "ability");
        let by_host = approval(Author::system("host"), "anything", None);
        assert_eq!(scope_choices(&by_host), ["once"]);
    }

    #[test]
    fn grants_are_kept_found_and_taken_back() {
        let (_d, s) = store();
        assert!(list(&s).unwrap().is_empty());
        let g = Grant {
            key: key("claude", "Install a package"),
            agent: "claude".into(),
            action: "Install a package".into(),
            approval: 4,
            by: "acct".into(),
            by_name: "Ada".into(),
            ts_ms: cua_volume::now_ms(),
            device: None,
            from_phone: false,
        };
        add(&s, g.clone()).unwrap();
        add(
            &s,
            Grant {
                approval: 9,
                ..g.clone()
            },
        )
        .unwrap();
        assert_eq!(
            list(&s).unwrap().len(),
            1,
            "the same request keeps the first"
        );
        assert_eq!(
            find(&s, "claude", "install a PACKAGE")
                .unwrap()
                .unwrap()
                .approval,
            4
        );
        assert!(find(&s, "codex", "install a package").unwrap().is_none());
        assert_eq!(revoke(&s, "claude", "install a package").unwrap(), 1);
        assert_eq!(revoke(&s, "claude", "install a package").unwrap(), 0);
        assert!(find(&s, "claude", "install a package").unwrap().is_none());
    }

    #[test]
    fn an_expired_standing_answer_does_not_stop_a_new_one_from_being_kept() {
        let (_d, s) = store();
        let old = cua_volume::now_ms().saturating_sub(TTL_MS + 1000);
        let g = |ts_ms: u64, approval: u64| Grant {
            key: key("claude", "Install a package"),
            agent: "claude".into(),
            action: "Install a package".into(),
            approval,
            by: "acct".into(),
            by_name: "Ada".into(),
            ts_ms,
            device: None,
            from_phone: false,
        };
        // Written a day ago (as it was, before it ended).
        save(&s, &[g(old, 1)]).unwrap();
        assert!(find(&s, "claude", "install a package").unwrap().is_none());
        add(&s, g(cua_volume::now_ms(), 2)).unwrap();
        let kept = find(&s, "claude", "install a package").unwrap().unwrap();
        assert_eq!(kept.approval, 2, "the new answer was kept");
        assert_eq!(list(&s).unwrap().len(), 1, "and the expired one is gone");
    }

    #[test]
    fn a_standing_answer_ends_after_a_day() {
        let (_d, s) = store();
        let then = 5_000_000_000u64;
        add(
            &s,
            Grant {
                key: key("claude", "Install a package"),
                agent: "claude".into(),
                action: "Install a package".into(),
                approval: 4,
                by: "acct".into(),
                by_name: "Ada".into(),
                ts_ms: then,
                device: None,
                from_phone: false,
            },
        )
        .unwrap();
        // The add above pruned nothing it just wrote, even at the test's clock.
        let inside = find_at(&s, "claude", "install a package", then + TTL_MS - 1).unwrap();
        assert!(inside.is_some());
        let gone = find_at(&s, "claude", "install a package", then + TTL_MS).unwrap();
        assert!(gone.is_none(), "the same request asks again after a day");
    }
}
