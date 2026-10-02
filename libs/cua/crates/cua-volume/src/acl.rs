// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who may read and write what.
//!
//! | Principal in a session | `public/` | `agents/<self>/` | `agents/<other>/` | `spaces/<this>/` | `spaces/<other>/` |
//! |---|---|---|---|---|---|
//! | user (app, CLI, host agents) | rw | rw | rw | rw | rw |
//! | agent A in Space S | r | rw | none | rw | none |
//! | Space S, no agent | r | none | none | rw | none |
//!
//! Everything past this table is a [`Grant`]: `{principal, prefix, mode,
//! expires}`, made only by the user and only with presence, revocable, and
//! audited.

use serde::{Deserialize, Serialize};

use crate::path::{self, Area};
use crate::{Error, Result};

/// Read, or read and write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Mode {
    #[serde(rename = "r")]
    Read,
    #[serde(rename = "rw")]
    ReadWrite,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Read => "r",
            Mode::ReadWrite => "rw",
        }
    }

    pub fn parse(s: &str) -> Result<Mode> {
        match s.trim() {
            "r" | "read" | "ro" => Ok(Mode::Read),
            "rw" | "write" | "read-write" => Ok(Mode::ReadWrite),
            other => Err(Error::Invalid(format!("mode {other:?}: use r or rw"))),
        }
    }

    /// Whether this mode allows `want`.
    pub fn allows(self, want: Mode) -> bool {
        self >= want
    }
}

/// Who is acting.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Principal {
    /// The account's user: the app, the CLI, and agents on the user's own
    /// machine acting for them.
    User,
    /// A named persistent agent.
    Agent(String),
    /// A Space with no agent (a shell in it), by Space id.
    Space(String),
}

impl Principal {
    /// `user`, `agent:ada`, `space:local-work` (the Space's folder name).
    pub fn id(&self) -> String {
        match self {
            Principal::User => "user".into(),
            Principal::Agent(a) => format!("agent:{a}"),
            Principal::Space(s) => format!("space:{}", path::space_folder_name(s)),
        }
    }

    /// Parses [`Principal::id`] (a Space may be given by id or folder name).
    pub fn parse(s: &str) -> Result<Principal> {
        let s = s.trim();
        if s == "user" {
            return Ok(Principal::User);
        }
        if let Some(a) = s.strip_prefix("agent:") {
            if !path::valid_agent_name(a) {
                return Err(Error::Invalid(format!("agent name {a:?}")));
            }
            return Ok(Principal::Agent(a.into()));
        }
        if let Some(sp) = s.strip_prefix("space:")
            && !sp.is_empty()
        {
            return Ok(Principal::Space(sp.into()));
        }
        Err(Error::Invalid(format!(
            "principal {s:?}: use user, agent:<name> or space:<id>"
        )))
    }
}

impl std::fmt::Display for Principal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.id())
    }
}

/// A session: a principal, and the Space it acts in (if any).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub principal: Principal,
    /// The Space id of the session (`local:work`).
    pub space: Option<String>,
}

impl Context {
    pub fn user() -> Context {
        Context {
            principal: Principal::User,
            space: None,
        }
    }

    pub fn agent(name: &str, space: Option<&str>) -> Context {
        Context {
            principal: Principal::Agent(name.into()),
            space: space.map(str::to_string),
        }
    }

    pub fn space(id: &str) -> Context {
        Context {
            principal: Principal::Space(id.into()),
            space: Some(id.into()),
        }
    }

    fn space_folder(&self) -> Option<String> {
        self.space.as_deref().map(path::space_folder_name)
    }
}

/// The access the defaults give `ctx` on a normalized key.
pub fn default_mode(ctx: &Context, key: &str) -> Option<Mode> {
    if ctx.principal == Principal::User {
        return Some(Mode::ReadWrite);
    }
    match path::area(key) {
        Area::Public => Some(Mode::Read),
        Area::Agent(a) => match &ctx.principal {
            Principal::Agent(me) if *me == a => Some(Mode::ReadWrite),
            _ => None,
        },
        Area::Space(s) => {
            (ctx.space_folder().as_deref() == Some(s.as_str())).then_some(Mode::ReadWrite)
        }
        Area::Root | Area::Other => None,
    }
}

/// A widening of the defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub id: String,
    /// `agent:<name>` or `space:<folder>`.
    pub principal: String,
    /// A folder (`agents/writer/outputs/`) or one key.
    pub prefix: String,
    pub mode: Mode,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<u64>,
    #[serde(default)]
    pub revoked: bool,
    /// Free text shown in the app.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

impl Grant {
    /// Live at `now_ms` (not revoked, not expired).
    pub fn is_live(&self, now_ms: u64) -> bool {
        !self.revoked && self.expires_ms.is_none_or(|e| now_ms < e)
    }

    /// Whether this grant covers `key` for `ctx`.
    pub fn covers(&self, ctx: &Context, key: &str) -> bool {
        let who = match Principal::parse(&self.principal) {
            Ok(p) => p,
            Err(_) => return false,
        };
        if who.id() != ctx.principal.id() {
            return false;
        }
        if self.prefix.ends_with('/') || self.prefix.is_empty() {
            key.starts_with(&self.prefix) || format!("{key}/") == self.prefix
        } else {
            key == self.prefix
        }
    }
}

/// The access `ctx` has on `key`: the defaults widened by live grants.
pub fn effective_mode(ctx: &Context, key: &str, grants: &[Grant], now_ms: u64) -> Option<Mode> {
    let mut best = default_mode(ctx, key);
    for g in grants {
        if g.is_live(now_ms) && g.covers(ctx, key) {
            best = best.max(Some(g.mode));
        }
    }
    best
}

/// Whether `ctx` can see into folder `prefix` at all (to list it): it has
/// access to the folder itself, to something under it, or to something
/// that contains it.
pub fn can_traverse(ctx: &Context, prefix: &str, grants: &[Grant], now_ms: u64) -> bool {
    if effective_mode(ctx, prefix, grants, now_ms).is_some() {
        return true;
    }
    let mut reachable = vec![];
    if let Principal::Agent(a) = &ctx.principal {
        reachable.push(format!("agents/{a}/"));
    }
    if let Some(s) = ctx.space_folder() {
        reachable.push(format!("spaces/{s}/"));
    }
    if ctx.principal != Principal::User {
        reachable.push(path::PUBLIC.into());
    }
    for g in grants {
        if g.is_live(now_ms)
            && Principal::parse(&g.principal).is_ok_and(|p| p.id() == ctx.principal.id())
        {
            reachable.push(g.prefix.clone());
        }
    }
    reachable.iter().any(|r| r.starts_with(prefix))
}

/// Validates a grant request and says whether it widens the defaults (a
/// grant that widens nothing is refused as pointless).
pub fn validate_grant(principal: &str, prefix: &str, mode: Mode) -> Result<(Principal, String)> {
    let who = Principal::parse(principal)?;
    if who == Principal::User {
        return Err(Error::Invalid("the user already has full access".into()));
    }
    let key = path::normalize(prefix)?;
    let ctx = match &who {
        Principal::Agent(a) => Context::agent(a, None),
        Principal::Space(s) => Context::space(s),
        Principal::User => unreachable!(),
    };
    if default_mode(&ctx, &key).is_some_and(|m| m.allows(mode)) {
        return Err(Error::Invalid(format!(
            "{} already has {} on {key} by default",
            who.id(),
            mode.as_str()
        )));
    }
    Ok((who, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(principal: &str, prefix: &str, mode: Mode) -> Grant {
        Grant {
            id: "g".into(),
            principal: principal.into(),
            prefix: prefix.into(),
            mode,
            created_ms: 0,
            expires_ms: None,
            revoked: false,
            note: String::new(),
        }
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn the_default_table() {
        let user = Context::user();
        let ada = Context::agent("ada", Some("local:work"));
        let shell = Context::space("local:work");
        let cases: &[(&str, Option<Mode>, Option<Mode>, Option<Mode>)] = &[
            (
                "public/a.md",
                Some(Mode::ReadWrite),
                Some(Mode::Read),
                Some(Mode::Read),
            ),
            (
                "agents/ada/memory/MEMORY.md",
                Some(Mode::ReadWrite),
                Some(Mode::ReadWrite),
                None,
            ),
            (
                "agents/bob/memory/MEMORY.md",
                Some(Mode::ReadWrite),
                None,
                None,
            ),
            (
                "spaces/local-work/out.txt",
                Some(Mode::ReadWrite),
                Some(Mode::ReadWrite),
                Some(Mode::ReadWrite),
            ),
            ("spaces/cloud-x/out.txt", Some(Mode::ReadWrite), None, None),
            ("other/x", Some(Mode::ReadWrite), None, None),
        ];
        for (key, u, a, s) in cases {
            assert_eq!(default_mode(&user, key), *u, "user {key}");
            assert_eq!(default_mode(&ada, key), *a, "agent {key}");
            assert_eq!(default_mode(&shell, key), *s, "space {key}");
        }
        // An agent outside any Space still owns its home, and no Space folder.
        let lone = Context::agent("ada", None);
        assert_eq!(default_mode(&lone, "spaces/local-work/x"), None);
        assert_eq!(default_mode(&lone, "agents/ada/x"), Some(Mode::ReadWrite));
    }

    #[test]
    fn grants_widen_and_expire() {
        let rs = Context::agent("researcher", Some("local:a"));
        let key = "agents/writer/outputs/report.md";
        assert_eq!(effective_mode(&rs, key, &[], 0), None);
        let grants = vec![g("agent:researcher", "agents/writer/outputs/", Mode::Read)];
        assert_eq!(effective_mode(&rs, key, &grants, 0), Some(Mode::Read));
        assert_eq!(
            effective_mode(&rs, "agents/writer/memory/x", &grants, 0),
            None
        );
        // Someone else's grant does not apply.
        let other = Context::agent("intern", None);
        assert_eq!(effective_mode(&other, key, &grants, 0), None);
        let mut expired = grants.clone();
        expired[0].expires_ms = Some(10);
        assert_eq!(effective_mode(&rs, key, &expired, 10), None);
        let mut revoked = grants;
        revoked[0].revoked = true;
        assert_eq!(effective_mode(&rs, key, &revoked, 0), None);
        // A grant can raise public/ to rw.
        let pubw = vec![g("agent:researcher", "public/datasets/", Mode::ReadWrite)];
        assert_eq!(
            effective_mode(&rs, "public/datasets/a.csv", &pubw, 0),
            Some(Mode::ReadWrite)
        );
        assert_eq!(
            effective_mode(&rs, "public/b.csv", &pubw, 0),
            Some(Mode::Read)
        );
    }

    #[test]
    fn traversal_follows_reachable_folders() {
        let ada = Context::agent("ada", Some("local:work"));
        assert!(can_traverse(&ada, "", &[], 0));
        assert!(can_traverse(&ada, "agents/", &[], 0));
        assert!(can_traverse(&ada, "spaces/", &[], 0));
        assert!(!can_traverse(&ada, "agents/bob/", &[], 0));
        let grants = vec![g("agent:ada", "agents/bob/outputs/", Mode::Read)];
        assert!(can_traverse(&ada, "agents/bob/", &grants, 0));
    }

    #[test]
    fn grants_must_widen() {
        assert!(validate_grant("agent:ada", "agents/ada/x", Mode::ReadWrite).is_err());
        assert!(validate_grant("agent:ada", "public/", Mode::Read).is_err());
        assert!(validate_grant("agent:ada", "public/", Mode::ReadWrite).is_ok());
        assert!(validate_grant("agent:ada", "agents/bob/", Mode::Read).is_ok());
        assert!(validate_grant("user", "agents/bob/", Mode::Read).is_err());
        assert!(validate_grant("agent:ada", "agents/../x", Mode::Read).is_err());
        assert_eq!(
            Principal::parse("space:local:work").unwrap().id(),
            "space:local-work"
        );
    }
}
