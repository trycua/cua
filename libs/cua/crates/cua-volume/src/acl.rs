// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Who may read and write what.
//!
//! | Principal in a session | `public/` | `agents/<self>/` | `agents/<other>/` | `spaces/<this>/` | `spaces/<this>/thread/`, `spaces/<this>/sessions/` | `spaces/<other>/` |
//! |---|---|---|---|---|---|---|
//! | user (app, CLI, host agents) | rw | rw | rw | rw | rw | rw |
//! | agent A in Space S | r | rw | none | rw | r | none |
//! | Space S, no agent | r | none | none | rw | r | none |
//!
//! Under Secure Spaces ([`Context::isolated`]) `public/` is not readable by
//! default: a Space (or its agent) reads it only through a grant, for the
//! Space itself (`space:<id>`) or for every Space of its tree
//! (`tree:<root>`), and the `public/` cells above are `none`. A Space's
//! folder is then `spaces/<name>--<id>/`, so a Space created later under
//! the same name starts empty.
//!
//! Everything past this table is a [`Grant`]: `{principal, prefix, mode,
//! expires}`, made only by the user and only with presence, revocable, and
//! audited. A Space's `thread/` and `sessions/` stay read-only for everyone
//! but the user whatever the grants say: only the owner daemon (as the user)
//! writes the thread's mirror and the session journals, so nothing inside the
//! Space can forge an entry through its mount.

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
    /// Every Space of one tree, by the tree's root Space id. Only grants
    /// name it: a session is never "a tree".
    Tree(String),
}

impl Principal {
    /// `user`, `agent:ada`, `space:local-work` (the Space's folder name).
    pub fn id(&self) -> String {
        match self {
            Principal::User => "user".into(),
            Principal::Agent(a) => format!("agent:{a}"),
            Principal::Space(s) => format!("space:{}", path::space_folder_name(s)),
            Principal::Tree(r) => format!("tree:{}", path::space_folder_name(r)),
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
        if let Some(r) = s.strip_prefix("tree:")
            && !r.is_empty()
        {
            return Ok(Principal::Tree(r.into()));
        }
        Err(Error::Invalid(format!(
            "principal {s:?}: use user, agent:<name>, space:<id> or tree:<root>"
        )))
    }
}

impl std::fmt::Display for Principal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.id())
    }
}

/// A folder a Space mounted: a named volume (`volumes/<name>/`, or a
/// folder inside it), at the given mode, in addition to the defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountAccess {
    /// A folder key ending in `/`.
    pub prefix: String,
    pub mode: Mode,
}

/// A session: a principal, and the Space it acts in (if any).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Context {
    pub principal: Principal,
    /// The Space id of the session (`local:work`).
    pub space: Option<String>,
    /// What the Space mounted besides its own folder. Never widens the
    /// user, and never outlives the Space's record.
    pub mounts: Vec<MountAccess>,
    /// The root of the Space's tree (a Space id). A `tree:<root>` grant
    /// applies to sessions in Spaces with this root.
    pub tree: Option<String>,
    /// Whether the Space has its own folder (the Space's Volume setting is
    /// "Own folder", the default). Off: the folder is out of reach, and so
    /// is everything else but the mounted volumes and grants.
    pub own_folder: bool,
    /// Secure Spaces: `public/` is not readable without a grant. Without it
    /// every Space reads `public/`, as it always could.
    pub isolated: bool,
}

impl Context {
    pub fn user() -> Context {
        Context {
            principal: Principal::User,
            space: None,
            mounts: vec![],
            tree: None,
            own_folder: true,
            isolated: false,
        }
    }

    pub fn agent(name: &str, space: Option<&str>) -> Context {
        Context {
            principal: Principal::Agent(name.into()),
            space: space.map(str::to_string),
            mounts: vec![],
            tree: None,
            own_folder: true,
            isolated: false,
        }
    }

    pub fn space(id: &str) -> Context {
        Context {
            principal: Principal::Space(id.into()),
            space: Some(id.into()),
            mounts: vec![],
            tree: None,
            own_folder: true,
            isolated: false,
        }
    }

    /// This session in the tree rooted at Space `root` (a Space is its own
    /// tree until it has a parent). Applies to a Space or an agent acting
    /// in one.
    pub fn with_tree(mut self, root: &str) -> Context {
        if self.principal != Principal::User && self.space.is_some() {
            self.tree = Some(root.to_string());
        }
        self
    }

    /// This session with the folders its Space mounted. Applies to a Space
    /// or an agent acting in one; the user already has everything.
    pub fn with_mounts(mut self, mounts: Vec<MountAccess>) -> Context {
        if self.principal != Principal::User && self.space.is_some() {
            self.mounts = mounts;
        }
        self
    }

    /// This session under Secure Spaces: `public/` needs a grant.
    pub fn isolated(mut self) -> Context {
        self.isolated = true;
        self
    }

    /// This session in a Space whose Volume is off: no folder of its own.
    pub fn without_own_folder(mut self) -> Context {
        self.own_folder = false;
        self
    }

    fn space_folder(&self) -> Option<String> {
        self.space.as_deref().map(path::space_folder_name)
    }

    /// The Space folder the defaults give this session (`None` when the
    /// Space's Volume is off).
    fn own_space_folder(&self) -> Option<String> {
        self.own_folder.then(|| self.space_folder()).flatten()
    }
}

/// The folders of a Space's folder only the user writes.
pub const DAEMON_ONLY: [&str; 2] = ["thread", "sessions"];

/// Whether `key` is inside a Space's `thread/` or `sessions/` (or is one of
/// those folders).
pub fn is_daemon_only(key: &str) -> bool {
    let Some(rest) = key.strip_prefix(path::SPACES) else {
        return false;
    };
    let Some((_, inner)) = rest.split_once('/') else {
        return false;
    };
    let first = inner.split('/').next().unwrap_or("");
    DAEMON_ONLY.contains(&first)
}

/// The access the defaults give `ctx` on a normalized key.
pub fn default_mode(ctx: &Context, key: &str) -> Option<Mode> {
    let mode = default_mode_inner(ctx, key);
    cap_daemon_only(ctx, key, mode)
}

/// Read at most on a daemon-only folder for anyone but the user.
fn cap_daemon_only(ctx: &Context, key: &str, mode: Option<Mode>) -> Option<Mode> {
    if ctx.principal != Principal::User && is_daemon_only(key) {
        mode.map(|m| m.min(Mode::Read))
    } else {
        mode
    }
}

fn default_mode_inner(ctx: &Context, key: &str) -> Option<Mode> {
    if ctx.principal == Principal::User {
        return Some(Mode::ReadWrite);
    }
    match path::area(key) {
        // Under Secure Spaces only a grant opens `public/` to a Space or an
        // agent.
        Area::Public => (!ctx.isolated).then_some(Mode::Read),
        Area::Agent(a) => match &ctx.principal {
            Principal::Agent(me) if *me == a => Some(Mode::ReadWrite),
            _ => None,
        },
        Area::Space(s) => {
            (ctx.own_space_folder().as_deref() == Some(s.as_str())).then_some(Mode::ReadWrite)
        }
        Area::Root => None,
        Area::Other => mounted_mode(ctx, key),
    }
}

/// The access the Space's mounts give on `key` (the best of those that
/// cover it).
fn mounted_mode(ctx: &Context, key: &str) -> Option<Mode> {
    ctx.mounts
        .iter()
        .filter(|m| key.starts_with(&m.prefix) || format!("{key}/") == m.prefix)
        .map(|m| m.mode)
        .max()
}

/// A widening of the defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub id: String,
    /// `agent:<name>`, `space:<folder>` or `tree:<root folder>`.
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

    /// Whether this grant is for the session `ctx`: the same agent or
    /// Space, a Space the session acts in (an agent in Space S has what a
    /// grant to S gives, since it shares S's guest), or the tree the
    /// session's Space is in.
    pub fn applies_to(&self, ctx: &Context) -> bool {
        let Ok(who) = Principal::parse(&self.principal) else {
            return false;
        };
        if ctx.principal == Principal::User {
            return false;
        }
        match who {
            Principal::User => false,
            Principal::Agent(_) => who.id() == ctx.principal.id(),
            Principal::Space(_) => ctx
                .space_folder()
                .is_some_and(|f| format!("space:{f}") == who.id()),
            Principal::Tree(_) => ctx
                .tree
                .as_deref()
                .is_some_and(|r| format!("tree:{}", path::space_folder_name(r)) == who.id()),
        }
    }

    /// Whether this grant covers `key` for `ctx`.
    pub fn covers(&self, ctx: &Context, key: &str) -> bool {
        if !self.applies_to(ctx) {
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
    cap_daemon_only(ctx, key, best)
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
    if let Some(s) = ctx.own_space_folder() {
        reachable.push(format!("spaces/{s}/"));
    }
    if ctx.principal != Principal::User && !ctx.isolated {
        reachable.push(path::PUBLIC.into());
    }
    reachable.extend(ctx.mounts.iter().map(|m| m.prefix.clone()));
    for g in grants {
        if g.is_live(now_ms) && g.applies_to(ctx) {
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
    if mode == Mode::ReadWrite && is_daemon_only(&key) {
        return Err(Error::Invalid(format!(
            "{key}: a Space's thread/ and sessions/ are written only by the Cua daemon"
        )));
    }
    // Judged as under Secure Spaces, where `public/` needs a grant: a read
    // grant on it widens there (and changes nothing without the experiment).
    let ctx = match &who {
        Principal::Agent(a) => Context::agent(a, None),
        Principal::Space(s) => Context::space(s),
        Principal::Tree(r) => Context::space(r).with_tree(r),
        Principal::User => unreachable!(),
    }
    .isolated();
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
    fn a_spaces_thread_and_sessions_are_read_only_inside_it() {
        let ada = Context::agent("ada", Some("local:work"));
        let shell = Context::space("local:work");
        let user = Context::user();
        for key in [
            "spaces/local-work/thread/entries/000000000001.json",
            "spaces/local-work/thread/members.json",
            "spaces/local-work/thread",
            "spaces/local-work/sessions/s1/000001/00000001.jsonl",
        ] {
            assert_eq!(default_mode(&ada, key), Some(Mode::Read), "agent {key}");
            assert_eq!(default_mode(&shell, key), Some(Mode::Read), "space {key}");
            assert_eq!(
                default_mode(&user, key),
                Some(Mode::ReadWrite),
                "user {key}"
            );
        }
        // The rest of the Space's folder stays read-write, and a file that
        // only starts with the word is not the folder.
        for key in ["spaces/local-work/out.txt", "spaces/local-work/threads.md"] {
            assert_eq!(default_mode(&ada, key), Some(Mode::ReadWrite), "{key}");
        }
        // Another Space's thread: nothing.
        assert_eq!(default_mode(&ada, "spaces/cloud-x/thread/a.json"), None);
        // No grant widens it.
        let grants = vec![g("agent:ada", "spaces/local-work/", Mode::ReadWrite)];
        assert_eq!(
            effective_mode(&ada, "spaces/local-work/thread/x.json", &grants, 0),
            Some(Mode::Read)
        );
        assert!(validate_grant("agent:ada", "spaces/local-work/thread/", Mode::ReadWrite).is_err());
        assert!(validate_grant("agent:bob", "spaces/local-work/thread/", Mode::Read).is_ok());
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
        // Under Secure Spaces public/ is not readable without a grant, and a
        // grant covers only its prefix.
        let iso = Context::agent("researcher", Some("local:w")).isolated();
        assert_eq!(effective_mode(&iso, "public/b.csv", &[], 0), None);
        let pubr = vec![g("agent:researcher", "public/", Mode::Read)];
        assert_eq!(
            effective_mode(&iso, "public/b.csv", &pubr, 0),
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
        assert!(validate_grant("agent:ada", "public/", Mode::Read).is_ok());
        assert!(validate_grant("agent:ada", "public/", Mode::ReadWrite).is_ok());
        assert!(validate_grant("tree:local:work--0a1b2c3d", "public/", Mode::Read).is_ok());
        assert!(validate_grant("tree:", "public/", Mode::Read).is_err());
        assert!(validate_grant("agent:ada", "agents/bob/", Mode::Read).is_ok());
        assert!(validate_grant("user", "agents/bob/", Mode::Read).is_err());
        assert!(validate_grant("agent:ada", "agents/../x", Mode::Read).is_err());
        assert_eq!(
            Principal::parse("space:local:work").unwrap().id(),
            "space:local-work"
        );
    }

    #[test]
    fn public_is_refused_without_a_grant_and_a_grant_opens_it_read_only() {
        let work = "local:work--0a1b2c3d";
        let shell = Context::space(work).isolated();
        let ada = Context::agent("ada", Some(work)).isolated();
        for ctx in [&shell, &ada] {
            assert_eq!(effective_mode(ctx, "public/a.md", &[], 0), None);
            assert!(!can_traverse(ctx, "public/", &[], 0));
        }
        let grants = vec![g(
            &Principal::Space(work.into()).id(),
            "public/",
            Mode::Read,
        )];
        // The Space's grant reaches the Space and the agent acting in it.
        for ctx in [&shell, &ada] {
            assert_eq!(
                effective_mode(ctx, "public/a.md", &grants, 0),
                Some(Mode::Read)
            );
            assert!(can_traverse(ctx, "public/", &grants, 0));
        }
        // Not another Space, and not a write.
        let other = Context::space("local:other--11111111").isolated();
        assert_eq!(effective_mode(&other, "public/a.md", &grants, 0), None);
        assert_eq!(
            effective_mode(&shell, "public/a.md", &grants, 0).map(|m| m.allows(Mode::ReadWrite)),
            Some(false)
        );
    }

    #[test]
    fn a_tree_grant_reaches_every_space_of_the_tree_only() {
        let root = "local:lead--0a1b2c3d";
        let child = "local:kid--0000beef";
        let a = Context::space(root).with_tree(root).isolated();
        let b = Context::agent("ada", Some(child))
            .with_tree(root)
            .isolated();
        let stranger = Context::space("local:x--99999999")
            .with_tree("local:x--99999999")
            .isolated();
        let untreed = Context::space(child).isolated();
        let grants = vec![g(&Principal::Tree(root.into()).id(), "public/", Mode::Read)];
        assert_eq!(effective_mode(&a, "public/x", &grants, 0), Some(Mode::Read));
        assert_eq!(effective_mode(&b, "public/x", &grants, 0), Some(Mode::Read));
        assert_eq!(effective_mode(&stranger, "public/x", &grants, 0), None);
        assert_eq!(effective_mode(&untreed, "public/x", &grants, 0), None);
        // The user's own session is not narrowed or widened by it.
        assert_eq!(
            effective_mode(&Context::user(), "public/x", &grants, 0),
            Some(Mode::ReadWrite)
        );
        // Revoked and expired tree grants stop at once.
        let mut revoked = grants.clone();
        revoked[0].revoked = true;
        assert_eq!(effective_mode(&a, "public/x", &revoked, 0), None);
        assert_eq!(
            Principal::parse("tree:local:lead--0a1b2c3d").unwrap().id(),
            "tree:local-lead--0a1b2c3d"
        );
    }

    #[test]
    fn one_space_cannot_reach_anothers_folder() {
        let a = Context::space("local:a--00000001");
        let b = Context::agent("bob", Some("local:b--00000002"));
        for key in [
            "spaces/local-a--00000001/out.txt",
            "spaces/local-a--00000001/",
        ] {
            assert_eq!(default_mode(&a, key), Some(Mode::ReadWrite), "{key}");
            assert_eq!(default_mode(&b, key), None, "{key}");
        }
        // The same name under another id is another folder.
        let reborn = Context::space("local:a--00000003");
        assert_eq!(
            default_mode(&reborn, "spaces/local-a--00000001/out.txt"),
            None
        );
        // A grant naming the old folder does not carry over to the new one.
        let grants = vec![g(
            "space:local-a--00000001",
            "spaces/local-a--00000001/",
            Mode::ReadWrite,
        )];
        assert_eq!(
            effective_mode(&reborn, "spaces/local-a--00000001/x", &grants, 0),
            None
        );
    }

    #[test]
    fn volume_off_takes_the_own_folder_away_but_not_mounts() {
        let work = "local:work--0a1b2c3d";
        let own = "spaces/local-work--0a1b2c3d/out.txt";
        let on = Context::space(work);
        assert_eq!(default_mode(&on, own), Some(Mode::ReadWrite));
        let off = Context::space(work)
            .without_own_folder()
            .with_mounts(vec![MountAccess {
                prefix: "volumes/data/".into(),
                mode: Mode::Read,
            }]);
        assert_eq!(default_mode(&off, own), None);
        assert!(!can_traverse(&off, "spaces/local-work--0a1b2c3d/", &[], 0));
        assert_eq!(default_mode(&off, "volumes/data/a"), Some(Mode::Read));
        let ada = Context::agent("ada", Some(work)).without_own_folder();
        assert_eq!(default_mode(&ada, own), None);
        assert_eq!(default_mode(&ada, "agents/ada/x"), Some(Mode::ReadWrite));
    }
}
