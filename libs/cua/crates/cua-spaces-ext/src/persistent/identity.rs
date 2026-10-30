// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who an agent member is, as the app draws it: where it came from (started
//! in the Space, or connected from a computer with `cua join`), its handle,
//! the name and icon it chose, its harness, and the parts it cannot set:
//! its owner, the computer it posts from and how it joined.
//!
//! The owner, the device and how it joined come from the member record
//! (who added it, who redeemed the code, the redeeming computer), never
//! from the agent. The agent sets only its display name, icon and reported
//! harness ([`Persistent::thread_join`]), within the name rules
//! ([`check_name`]).
//!
//! A member connected from a computer holds a member key: a random secret
//! made when its join code is redeemed, kept only in that computer's
//! keychain. The record keeps its SHA-256 and the device it is bound to;
//! removing the member (or its device) drops both, so the key stops at once.

use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use super::AgentRecord;
use crate::thread::Author;

/// The longest display name.
pub const MAX_NAME: usize = 32;
/// The prefix of a member key.
pub const KEY_PREFIX: &str = "cuamk_";

/// Started inside the Space, or connected from a computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Space,
    External,
}

/// A person, as the record names them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Person {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

impl From<&Author> for Person {
    fn from(a: &Author) -> Person {
        Person {
            id: a.id.clone(),
            name: a.name.clone(),
        }
    }
}

/// The computer an external member posts from (never an address).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// The member's harness: known (started by Cua) or as the agent reported it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HarnessInfo {
    /// `hermes`, `claude-code`, `codex`, `cursor`, `openclaw`, `t3code` or
    /// `other`.
    pub id: String,
    #[serde(default)]
    pub version: Option<String>,
    /// The agent said so (unverified).
    #[serde(default)]
    pub reported: bool,
}

/// How and when it joined, and who let it in.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Joined {
    /// `started` (agent_add) or `prompt` (a join code).
    pub method: String,
    pub at_ms: u64,
    pub by: Person,
}

/// The identity a member record keeps (`identity` in `agents.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    pub origin: Origin,
    /// A stable id for the membership (its keychain entry is named after
    /// it, so a handle change keeps the key).
    #[serde(default)]
    pub member_id: String,
    pub display_name: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub harness: HarnessInfo,
    pub owner: Person,
    #[serde(default)]
    pub device: Option<Device>,
    pub joined: Joined,
    /// Redeemed, waiting for its join request to be answered.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    /// The handle is the one given at redeem time; its first `thread_join`
    /// picks the real one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub provisional: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_ms: Option<u64>,
    /// The redeeming computer's device key (base64url): every call with the
    /// member key must be signed by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_public_key: Option<String>,
    /// SHA-256 (hex) of the member key (external members).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_sha256: Option<String>,
    /// The join code it was redeemed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_id: Option<String>,
    /// The `join_request` entry it waits on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_seq: Option<u64>,
}

/// The identity as the tools show it (member rows, thread authors). Never
/// carries the key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Identity {
    pub origin: Origin,
    pub handle: String,
    pub display_name: String,
    /// Another member of the Space shows the same display name (rows only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collides: Option<bool>,
    #[serde(default)]
    pub icon: Option<String>,
    pub harness: HarnessInfo,
    pub owner: Person,
    #[serde(default)]
    pub device: Option<Device>,
    pub joined: Joined,
    /// `active` or `pending`.
    pub state: String,
    #[serde(default)]
    pub last_seen_ms: Option<u64>,
}

/// The harness id the app knows for a Cua harness id.
pub fn harness_id(harness: &str) -> &'static str {
    match harness {
        "hermes" => "hermes",
        "claude-code" | "claude" => "claude-code",
        "openai-codex" | "codex" => "codex",
        "cursor" | "cursor-agent" => "cursor",
        "openclaw" => "openclaw",
        "t3code" | "t3-code" => "t3code",
        _ => "other",
    }
}

/// `rec`'s stored identity, or for a member from before identities (a
/// started agent) the one it is read as.
pub fn stored_of(rec: &AgentRecord, owner: &Author) -> Stored {
    if let Some(s) = &rec.identity {
        return s.clone();
    }
    let by = Person::from(rec.added_by.as_ref().unwrap_or(owner));
    Stored {
        origin: Origin::Space,
        member_id: format!("mem_{}", rec.name),
        display_name: rec.handle().to_string(),
        icon: Some("harness".into()),
        harness: HarnessInfo {
            id: harness_id(&rec.harness).into(),
            version: None,
            reported: false,
        },
        owner: by.clone(),
        device: None,
        joined: Joined {
            method: "started".into(),
            at_ms: if rec.member_since_ms > 0 {
                rec.member_since_ms
            } else {
                rec.created_ms
            },
            by,
        },
        pending: false,
        provisional: false,
        last_seen_ms: None,
        device_public_key: None,
        key_sha256: None,
        code_id: None,
        request_seq: None,
    }
}

/// `rec`'s identity as shown (`collides` unset; [`with_collisions`] sets
/// it over a Space's members).
pub fn identity_of(rec: &AgentRecord, owner: &Author) -> Identity {
    let s = stored_of(rec, owner);
    Identity {
        origin: s.origin,
        handle: rec.handle().to_string(),
        display_name: s.display_name,
        collides: None,
        icon: s.icon,
        harness: s.harness,
        owner: s.owner,
        device: s.device,
        joined: s.joined,
        state: if s.pending { "pending" } else { "active" }.into(),
        last_seen_ms: s.last_seen_ms,
    }
}

/// Whether `rec` was connected from a computer.
pub fn is_external(rec: &AgentRecord) -> bool {
    rec.identity
        .as_ref()
        .is_some_and(|s| s.origin == Origin::External)
}

/// Sets `collides` on each identity: true when another one shows the same
/// display name (case-insensitive).
pub fn with_collisions(ids: &mut [Identity]) {
    let names: Vec<String> = ids.iter().map(|i| fold(&i.display_name)).collect();
    for (i, id) in ids.iter_mut().enumerate() {
        let me = &names[i];
        id.collides = Some(names.iter().enumerate().any(|(j, n)| j != i && n == me));
    }
}

fn fold(s: &str) -> String {
    s.trim().to_lowercase()
}

/// A display name the agent asked for, checked: refused when empty, longer
/// than [`MAX_NAME`], holding control characters, "Cua", a person's name
/// in the Space (`people`) or another member's handle (`handles`). Returns
/// the trimmed name, or the one-line reason.
pub fn check_name(name: &str, people: &[String], handles: &[String]) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("a name is required".into());
    }
    if n.chars().count() > MAX_NAME {
        return Err(format!("names are at most {MAX_NAME} characters"));
    }
    if n.chars().any(char::is_control) {
        return Err("names can't hold control characters".into());
    }
    let f = fold(n);
    if f == "cua" {
        return Err("\"Cua\" is the app's own name; pick another".into());
    }
    if people.iter().any(|p| fold(p) == f) {
        return Err(format!(
            "{n:?} is a person in this Space; pick another name"
        ));
    }
    let bare = f.trim_start_matches('@');
    if handles.iter().any(|h| fold(h) == bare) {
        return Err(format!(
            "{n:?} is another member's handle; pick another name"
        ));
    }
    Ok(n.to_string())
}

/// The handle a display name becomes (`a-z`, `0-9`, `.`, `_`, `-`; at most
/// 32), before collisions: "Claude Code" -> `claude-code`.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.') {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out: String = out
        .trim_matches(|c| c == '-' || c == '.')
        .chars()
        .take(MAX_NAME)
        .collect();
    let out = out.trim_end_matches(['-', '.']).to_string();
    if out.is_empty() {
        "agent".into()
    } else if !out.starts_with(|c: char| c.is_ascii_alphanumeric()) {
        format!("agent-{out}")
    } else {
        out
    }
}

/// The first free handle from `base` (`codex`, `codex-2`, ...), where
/// `taken` says whether a handle is in use.
pub fn free_handle(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) && cua_volume::path::valid_agent_name(base) {
        return base.to_string();
    }
    for n in 2.. {
        let suffix = format!("-{n}");
        let stem: String = base.chars().take(MAX_NAME - suffix.len()).collect();
        let h = format!("{}{suffix}", stem.trim_end_matches(['-', '.']));
        if !taken(&h) && cua_volume::path::valid_agent_name(&h) {
            return h;
        }
    }
    unreachable!("an unbounded range")
}

/// A new member key (only its hash is kept).
pub fn new_key() -> String {
    format!("{KEY_PREFIX}{}", hex::encode(rand::random::<[u8; 32]>()))
}

/// SHA-256 (hex) of a secret.
pub fn sha256_hex(s: &str) -> String {
    hex::encode(sha2::Sha256::digest(s.as_bytes()))
}

/// Constant-time equality of two hex digests.
pub fn same_digest(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked() {
        let people = vec!["Ada".to_string()];
        let handles = vec!["codex".to_string()];
        let ok = |n: &str| check_name(n, &people, &handles);
        assert_eq!(ok("  Codex CLI ").unwrap(), "Codex CLI");
        for bad in [
            "",
            "   ",
            "cua",
            "CUA",
            " Cua ",
            "ada",
            "Codex",
            "@codex",
            "tab\there",
            "a\u{7}",
            &"x".repeat(33),
        ] {
            assert!(ok(bad).is_err(), "{bad:?} should be refused");
        }
        assert!(ok(&"x".repeat(32)).is_ok());
    }

    #[test]
    fn handles_come_from_names() {
        assert_eq!(slug("Claude Code"), "claude-code");
        assert_eq!(slug("Codex"), "codex");
        assert_eq!(slug("  --Hermes!!  "), "hermes");
        assert_eq!(slug("日本"), "agent");
        assert_eq!(slug("_x"), "agent-_x");
        assert!(cua_volume::path::valid_agent_name(&slug("A b c")));
        let taken = ["codex", "codex-2"];
        assert_eq!(free_handle("codex", |h| taken.contains(&h)), "codex-3");
        assert_eq!(free_handle("hermes", |h| taken.contains(&h)), "hermes");
        let long = "x".repeat(32);
        let h = free_handle(&long, |h| h == long);
        assert!(h.len() <= 32 && h.ends_with("-2"), "{h}");
    }

    #[test]
    fn collisions_are_case_insensitive_and_symmetric() {
        let mk = |n: &str| Identity {
            origin: Origin::External,
            handle: n.to_lowercase(),
            display_name: n.into(),
            collides: None,
            icon: None,
            harness: HarnessInfo {
                id: "other".into(),
                version: None,
                reported: false,
            },
            owner: Person {
                id: "a".into(),
                name: "A".into(),
            },
            device: None,
            joined: Joined {
                method: "prompt".into(),
                at_ms: 0,
                by: Person {
                    id: "a".into(),
                    name: "A".into(),
                },
            },
            state: "active".into(),
            last_seen_ms: None,
        };
        let mut ids = vec![mk("Claude Code"), mk("claude code"), mk("Hermes")];
        with_collisions(&mut ids);
        assert_eq!(
            ids.iter().map(|i| i.collides).collect::<Vec<_>>(),
            vec![Some(true), Some(true), Some(false)]
        );
    }

    #[test]
    fn keys_are_random_and_compared_by_digest() {
        let (a, b) = (new_key(), new_key());
        assert_ne!(a, b);
        assert!(a.starts_with(KEY_PREFIX) && a.len() == KEY_PREFIX.len() + 64);
        assert!(same_digest(&sha256_hex(&a), &sha256_hex(&a)));
        assert!(!same_digest(&sha256_hex(&a), &sha256_hex(&b)));
        assert!(!same_digest("ab", "abc"));
        assert_eq!(harness_id("openai-codex"), "codex");
        assert_eq!(harness_id("gemini-cli"), "other");
    }
}
