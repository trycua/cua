// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Tools the host runs for an agent, behind a question to its owner.
//!
//! An agent in a Space holds no credential for the outside world. When it
//! wants something that leaves the Space (a push to a GitHub repository,
//! a pull request), it asks through its bridge and the *daemon* does the
//! work, on the host, with a connection only the host holds. The question
//! is posted by the daemon, not by the agent: the card's wording comes from
//! what the host measured (the files that changed, the repository and the
//! branch), and only an answer written by a person releases the action.
//! An export is never remembered: every push asks again. Every ask,
//! answer, refusal and push is a line in a hash-chained audit log.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{Value, json};

use cua_spaces::agents::github_auth::{self, Plan, Token, Unavailable};
use cua_spaces::{Error, Result};

use crate::thread::{Approval, Author, AuthorKind, Draft, EntryKind, ThreadStore};

/// The action class of a card the host posts for something that leaves a
/// Space. Clients show it with only "Push and open" and "Not now".
pub const EXPORT_PREFIX: &str = "export:";
/// The push-to-GitHub action.
pub const REPO_PUSH: &str = "export:repo.push";
/// The saved key (in the daemon's key store, host only) that is the
/// GitHub connection.
pub const GITHUB_KEY: &str = "GITHUB_TOKEN";
/// The saved key (host only) that is a deploy key of one repository: a private
/// SSH key with write access to that repository alone. It can push a branch
/// but cannot open a pull request (GitHub gives a deploy key no API access),
/// so the host hands back the link that opens it.
pub const DEPLOY_KEY: &str = "GITHUB_DEPLOY_KEY";
/// GitHub's published SSH host key, so a push never trusts a key it was not given.
const GITHUB_HOST_KEY: &str =
    "github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl\n";
/// How long the host waits for the owner.
pub const ASK_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// What the owner said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Approved,
    Declined(String),
}

/// Posts an approval entry written by the daemon and waits for a person's
/// answer to it. An answer by an agent or the daemon is ignored; so is a
/// stale one. Nothing is ever approved on a timeout.
pub async fn ask_owner(
    store: &ThreadStore,
    action: &str,
    text: &str,
    details: &str,
    timeout: Duration,
) -> Result<(u64, Decision)> {
    ask_owner_card(store, action, text, details, None, timeout).await
}

/// [`ask_owner`] with the card's data (a push's diffstat, commits and
/// branches, see `push_card`) for the apps to draw. `text` and `details`
/// stay for a client that does not know cards.
pub async fn ask_owner_card(
    store: &ThreadStore,
    action: &str,
    text: &str,
    details: &str,
    card: Option<serde_json::Value>,
    timeout: Duration,
) -> Result<(u64, Decision)> {
    let mut d = Draft::new(Author::system("host"), EntryKind::Approval, text);
    d.approval = Some(Approval {
        action: action.into(),
        details: details.into(),
        state: "open".into(),
        card,
    });
    let seq = store.append(d)?.seq;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut after = seq;
    loop {
        for e in store.read(after, 256)? {
            after = after.max(e.seq);
            if e.kind != EntryKind::Answer || e.author.kind != AuthorKind::Person {
                continue;
            }
            if let Some(a) = e.answer.filter(|a| a.approval == seq) {
                return Ok((
                    seq,
                    if a.approve {
                        Decision::Approved
                    } else {
                        Decision::Declined(a.note)
                    },
                ));
            }
        }
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(Error::invalid(
                "the owner did not answer in time; nothing was sent",
            ));
        }
        store
            .wait_after(after, (deadline - now).min(Duration::from_secs(30)))
            .await?;
    }
}

/// How a push is signed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// A token (saved, in the daemon's environment, or from `gh`): the host can push
    /// and open the pull request.
    Token,
    /// A deploy key of the repository: it pushes a branch and cannot open the pull request.
    DeployKey,
    /// Git's own sign-in on this computer (an SSH key or a credential helper).
    GitOwn,
    /// A local folder of bare repositories (tests and developer builds only): nothing to sign in to.
    Local,
}

/// How a token is read for an approved push (the chain's `token`).
pub type TokenFn = std::sync::Arc<
    dyn Fn(&str) -> std::result::Result<github_auth::Resolved, Unavailable> + Send + Sync,
>;

/// The GitHub connection the host holds, as the GitHub sign-in chain
/// (`cua_spaces::agents::github_auth`) describes it. It holds no token: the
/// token is read when an approved push runs ([`GithubConnection::token_now`]).
#[derive(Clone)]
#[cfg_attr(any(test, feature = "dev-remotes"), derive(Default))]
pub struct GithubConnection {
    /// A token given explicitly (a test's, or a caller that holds its own).
    token: Option<String>,
    deploy_key: Option<String>,
    /// The GitHub host (`github.com`, or a GitHub Enterprise host).
    pub host: String,
    /// What the chain says it will use for `host` (no token in it).
    plan: Option<Plan>,
    /// Whether git has a credential helper (`None`: ask git). A test's answer.
    git_helper: Option<bool>,
    /// Reads the token for an approved push (`None`: this process's chain). A test's.
    token_fn: Option<TokenFn>,
    /// Why the chain cannot be used (the person switched the host off, nothing is signed in).
    unavailable: Option<Unavailable>,
    /// Why a push is refused before anything is read (a developer override
    /// of where pushes go, in a build that does not allow one).
    refused: Option<String>,
    /// Where an SSH push goes (`ssh://git@github.com`).
    pub(crate) ssh_base: String,
    /// Where repositories are (`https://github.com`). Only a build with the
    /// `dev-remotes` feature (and the unit tests) can point it elsewhere.
    pub(crate) remote_base: String,
    /// The REST API (`https://api.github.com`).
    pub(crate) api_base: String,
}

/// Where a push and its pull request go. A shipped build only ever has
/// GitHub's own addresses ([`Endpoints::github`]); redirecting them (to a
/// local folder of bare repositories, a fake API) needs the `dev-remotes`
/// feature, which the Cua daemon's release build does not enable. Tests
/// build with it and pass the addresses in
/// ([`GithubConnection::from_host_with`]); no environment variable moves
/// an approved push in a release build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoints {
    pub ssh_base: String,
    pub remote_base: String,
    pub api_base: String,
}

/// The variables a developer build reads to redirect pushes.
const DEV_ENV: [&str; 3] = [
    "CUA_GITHUB_SSH_BASE",
    "CUA_GITHUB_REMOTE_BASE",
    "CUA_GITHUB_API_BASE",
];

impl Endpoints {
    /// GitHub's own addresses for `host`.
    pub fn github(host: &str) -> Endpoints {
        Endpoints {
            ssh_base: format!("ssh://git@{host}"),
            remote_base: format!("https://{host}"),
            api_base: github_auth::api_base(host),
        }
    }

    /// Other addresses (a local folder of bare repositories, a fake API).
    #[cfg(any(test, feature = "dev-remotes"))]
    pub fn custom(ssh_base: &str, remote_base: &str, api_base: &str) -> Endpoints {
        Endpoints {
            ssh_base: ssh_base.into(),
            remote_base: remote_base.into(),
            api_base: api_base.into(),
        }
    }

    /// The addresses for this process. A build with `dev-remotes` reads
    /// `CUA_GITHUB_*_BASE` (a local remote for tests). Any other build never
    /// reads them and says so when one is set: an environment variable must
    /// not decide where an approved token is sent.
    pub fn from_process(host: &str) -> std::result::Result<Endpoints, String> {
        Endpoints::from_vars(host, |n| std::env::var(n).ok().filter(|v| !v.is_empty()))
    }

    fn from_vars(
        host: &str,
        var: impl Fn(&str) -> Option<String>,
    ) -> std::result::Result<Endpoints, String> {
        let mut e = Endpoints::github(host);
        let set: Vec<&str> = DEV_ENV
            .iter()
            .copied()
            .filter(|n| var(n).is_some())
            .collect();
        if set.is_empty() {
            return Ok(e);
        }
        #[cfg(feature = "dev-remotes")]
        {
            if let Some(v) = var("CUA_GITHUB_SSH_BASE") {
                e.ssh_base = v;
            }
            if let Some(v) = var("CUA_GITHUB_REMOTE_BASE") {
                e.remote_base = v;
            }
            if let Some(v) = var("CUA_GITHUB_API_BASE") {
                e.api_base = v;
            }
            Ok(e)
        }
        #[cfg(not(feature = "dev-remotes"))]
        {
            let _ = &mut e;
            Err(format!(
                "{} would change where pushes go, and this build of Cua does not allow that: unset {}. Nothing was sent",
                set.join(" and "),
                if set.len() == 1 { "it" } else { "them" }
            ))
        }
    }
}

impl std::fmt::Debug for GithubConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubConnection")
            .field("host", &self.host)
            .field("plan", &self.plan)
            .field("token", &self.token.as_ref().map(|_| "(hidden)"))
            .field("deploy_key", &self.deploy_key.as_ref().map(|_| "(hidden)"))
            .field("custom_token_fn", &self.token_fn.is_some())
            .finish()
    }
}

impl GithubConnection {
    /// The connection for `host` from the daemon's sign-in chain, pushing to
    /// GitHub's own addresses. A release build refuses (at push time, in
    /// plain words) when `CUA_GITHUB_*_BASE` is set; see [`Endpoints`].
    pub fn from_host(host: &str) -> GithubConnection {
        let host = if github_auth::valid_host(host) {
            host
        } else {
            github_auth::DEFAULT_HOST
        };
        match Endpoints::from_process(host) {
            Ok(e) => GithubConnection::from_host_with(host, e),
            Err(why) => {
                let mut c = GithubConnection::from_host_with(host, Endpoints::github(host));
                c.refused = Some(why);
                c
            }
        }
    }

    /// [`GithubConnection::from_host`] with the addresses given.
    pub fn from_host_with(host: &str, endpoints: Endpoints) -> GithubConnection {
        let host = if github_auth::valid_host(host) {
            host
        } else {
            github_auth::DEFAULT_HOST
        };
        let deploy_key = cua_spaces::agents::keys::global()
            .values(&[DEPLOY_KEY])
            .remove(DEPLOY_KEY)
            .filter(|t| t.contains("PRIVATE KEY"));
        let mut c = GithubConnection {
            token: None,
            deploy_key,
            host: host.into(),
            plan: None,
            git_helper: None,
            token_fn: None,
            unavailable: None,
            refused: None,
            ssh_base: endpoints.ssh_base,
            remote_base: endpoints.remote_base,
            api_base: endpoints.api_base,
        };
        // A local remote has nothing to sign in to: the chain (and `gh`) is not asked.
        if !c.is_local() {
            match github_auth::global().plan(host) {
                Ok(p) => c.plan = Some(p),
                Err(u) => c.unavailable = Some(u),
            }
        }
        c
    }

    pub fn with_token(mut self, token: &str) -> Self {
        self.token = Some(token.into());
        self
    }

    pub fn with_deploy_key(mut self, key: &str) -> Self {
        self.deploy_key = Some(key.into());
        self
    }

    /// Says whether git has a credential helper, instead of asking git (a test's).
    pub fn with_git_helper(mut self, has: bool) -> Self {
        self.git_helper = Some(has);
        self
    }

    /// Reads the token for an approved push with `f` instead of this process's chain (a test's).
    pub fn with_token_fn(mut self, f: TokenFn) -> Self {
        self.token_fn = Some(f);
        self
    }

    /// A connection as the chain's `plan` describes it (a test's).
    pub fn with_plan(mut self, plan: Plan) -> Self {
        self.plan = Some(plan);
        self.unavailable = None;
        self
    }

    /// A local folder of bare repositories stands in for GitHub: only in
    /// tests and developer builds. A release build never takes this branch.
    #[cfg(any(test, feature = "dev-remotes"))]
    fn is_local(&self) -> bool {
        !self.remote_base.starts_with("http")
    }

    #[cfg(not(any(test, feature = "dev-remotes")))]
    fn is_local(&self) -> bool {
        false
    }

    /// A push that is refused before anything is read or measured (a
    /// developer override of where pushes go, in a build that does not allow
    /// one), in the words the person is shown.
    pub fn blocked(&self) -> Option<Error> {
        self.refused.as_ref().map(|m| Error::invalid(m.clone()))
    }

    /// The person switched this host off: nothing is sent, whatever else is set.
    pub fn is_off(&self) -> bool {
        matches!(self.unavailable, Some(Unavailable::Off(_)))
    }

    /// How a push would be signed in, or `None` when it cannot be. `ssh_remote`:
    /// the repository's own remote is an SSH one.
    pub fn method(&self, ssh_remote: bool) -> Option<Method> {
        if self.is_off() || self.refused.is_some() {
            return None;
        }
        if self.token.is_some() || self.plan.is_some() {
            Some(Method::Token)
        } else if self.deploy_key.is_some() {
            Some(Method::DeployKey)
        } else if self.is_local() {
            Some(Method::Local)
        } else if ssh_remote || self.git_helper.unwrap_or_else(git_has_credential_helper) {
            Some(Method::GitOwn)
        } else {
            None
        }
    }

    /// Whether a push can be made.
    pub fn ready(&self, ssh_remote: bool) -> bool {
        self.method(ssh_remote).is_some()
    }

    /// Whether the host can open a pull request (only a token can).
    pub fn can_open_pr(&self, ssh_remote: bool) -> bool {
        self.method(ssh_remote) == Some(Method::Token)
    }

    /// The account the card names, in plain words (no token is read for it).
    pub fn account(&self, ssh_remote: bool) -> String {
        match self.method(ssh_remote) {
            Some(Method::Token) => match &self.plan {
                Some(p) => p.account.clone(),
                None => "a GitHub token".into(),
            },
            Some(Method::DeployKey) => "a deploy key of this repository".into(),
            Some(Method::GitOwn) => {
                "this computer's own git sign-in (SSH key or credential helper)".into()
            }
            Some(Method::Local) => "a local folder".into(),
            None => "no GitHub sign-in".into(),
        }
    }

    /// The refusal for a push that cannot be signed in, in the words the person is shown.
    pub fn refusal(&self) -> Error {
        if let Some(e) = self.blocked() {
            return e;
        }
        match &self.unavailable {
            Some(u) => u.clone().into(),
            None => Error::invalid(format!(
                "{}; nothing was sent",
                github_auth::nothing_line(&self.host)
            )),
        }
    }

    /// The token for the push that was just approved, read now from the chain (for
    /// `gh`: `gh auth token`), or `None` when this push uses no token. The caller uses it
    /// for the push and the pull request and drops it.
    pub async fn token_now(&self) -> Result<Option<Token>> {
        if let Some(t) = &self.token {
            return Ok(Some(Token::new(t.clone())));
        }
        if self.plan.is_none() {
            return Ok(None);
        }
        let host = self.host.clone();
        let f = self.token_fn.clone();
        let r = tokio::task::spawn_blocking(move || match f {
            Some(f) => f(&host),
            None => github_auth::global().token(&host),
        })
        .await
        .map_err(|e| Error::invalid(e.to_string()))?;
        match r {
            Ok(r) => Ok(Some(r.token)),
            Err(u) => Err(u.into()),
        }
    }

    fn basic(token: &Token) -> String {
        format!(
            "Authorization: Basic {}",
            base64::engine::general_purpose::STANDARD
                .encode(format!("x-access-token:{}", token.expose()))
        )
    }
}

/// Whether git on this computer has a credential helper (the macOS Keychain's, Git
/// Credential Manager, `gh auth setup-git`): it is what an HTTPS push signs in with
/// when no token is given.
fn git_has_credential_helper() -> bool {
    std::process::Command::new("git")
        .args(["config", "--get-all", "credential.helper"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| !String::from_utf8_lossy(&o.stdout).trim().is_empty())
        .unwrap_or(false)
}

/// A push the agent asked for.
#[derive(Clone, Debug)]
pub struct PushRequest {
    pub repo: String,
    pub branch: String,
    pub title: String,
    pub body: String,
    pub open_pr: bool,
    pub base: Option<String>,
    /// The GitHub host (`github.com`, or a GitHub Enterprise host).
    pub host: String,
    /// The repository's own remote is an SSH one (what the host measured in the Space).
    pub ssh_remote: bool,
}

pub fn valid_repo(s: &str) -> bool {
    let ok = |p: &str| {
        !p.is_empty()
            && p.len() <= 100
            && p != "."
            && p != ".."
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    };
    s.split_once('/')
        .is_some_and(|(o, n)| ok(o) && ok(n) && !n.contains('/'))
}

pub fn valid_branch(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with(['-', '/'])
        && !s.ends_with(['/', '.'])
        && !s.contains("..")
        && !s.contains("//")
        && !s.ends_with(".lock")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
}

/// Quotes `s` for a POSIX shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// What the host measured in the Space before asking.
#[derive(Clone, Debug, Default)]
pub struct Measured {
    /// The full commit hash the push is of. The card, the bundle and the
    /// push are all bound to it.
    pub head: String,
    /// The full hash of the commit the card compares with (the merge base
    /// with the remote's head, or the commit before), or the empty tree's
    /// when the history has one commit.
    pub base: String,
    pub stat: String,
    pub files: usize,
    pub additions: u64,
    pub deletions: u64,
    /// Commits in `base..head`.
    pub commits: u64,
    /// The repository's origin is an SSH remote (the address itself is never kept: it can hold a token).
    pub ssh_remote: bool,
}

/// Git's flags that make a diffstat the same wherever it is counted: no
/// rename detection, git's own algorithm, no external diff or text
/// conversion, no attributes of the work tree.
const STAT_FLAGS: &str = "--no-ext-diff --no-textconv --no-renames --diff-algorithm=myers";
/// The empty tree: attributes are read from it, so a repository's own
/// `.gitattributes` cannot change what a diffstat counts.
pub const EMPTY_TREE_SHA1: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Whether `s` is a full object name (40 or 64 hex digits).
pub fn is_full_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// What the card showed, in numbers git can recompute from the commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub head: String,
    pub base: String,
    pub files: u64,
    pub additions: u64,
    pub deletions: u64,
    pub commits: u64,
}

impl From<&Measured> for Binding {
    fn from(m: &Measured) -> Binding {
        Binding {
            head: m.head.clone(),
            base: m.base.clone(),
            files: m.files as u64,
            additions: m.additions,
            deletions: m.deletions,
            commits: m.commits,
        }
    }
}

/// Measures the repository at `path` in `space` and bundles its `HEAD` to
/// `bundle` (a guest path). Read-only for the repository. Every number is
/// measured at one commit, named in full (`Measured::head`), so what the
/// card shows can be recomputed from that commit alone.
///
/// The bundle is not frozen: the agent shares the Space's user and could
/// replace it whatever its mode. The host's check after the answer
/// ([`verify_bundle`]) is what holds, not this file.
pub async fn measure_in_space(
    space: &cua_spaces::Space,
    path: &str,
    bundle: &str,
) -> Result<Measured> {
    let out = space
        .bash(&measure_script(path, bundle), Duration::from_secs(120))
        .await?;
    parse_measured(path, &out.stdout, &out.stderr, out.exit_code)
}

/// The shell script that measures the repository and bundles its `HEAD`.
fn measure_script(path: &str, bundle: &str) -> String {
    let p = sh_quote(path);
    let b = sh_quote(bundle);
    format!(
        "set -e; cd {p}; git rev-parse --is-inside-work-tree >/dev/null; \
         if [ -n \"$(git status --porcelain)\" ]; then echo UNCOMMITTED; exit 3; fi; \
         head=$(git rev-parse --verify 'HEAD^{{commit}}'); \
         base=$(git merge-base \"$head\" origin/HEAD 2>/dev/null || git rev-parse --verify \"$head~1\" 2>/dev/null || git hash-object -t tree /dev/null); \
         echo \"HEAD $head\"; echo \"BASE $base\"; \
         case \"$(git remote get-url origin 2>/dev/null | head -n 1)\" in git@*|ssh://*) echo REMOTE_SSH;; esac; \
         git -c core.attributesFile=/dev/null -c attr.tree={EMPTY_TREE_SHA1} diff {STAT_FLAGS} --shortstat \"$base\" \"$head\" | sed 's/^/STAT /'; \
         if git cat-file -e \"$base^{{commit}}\" 2>/dev/null; then range=\"$base..$head\"; else range=\"$head\"; fi; \
         echo \"COMMITS $(git rev-list --count \"$range\")\"; \
         git bundle create {b} HEAD >/dev/null 2>&1; \
         now=$(git bundle list-heads {b} | awk '$2==\"HEAD\"{{print $1}}'); \
         if [ \"$now\" != \"$head\" ]; then echo MOVED; exit 4; fi"
    )
}

/// The shell script that bundles commit `head` of the repository at
/// `path` (under a temporary ref, removed again) to `bundle`, and measures
/// `base..head` the way [`measure_script`] does, for a viewer of a commit
/// the checkout may have moved on from. The two hashes are full names
/// (checked by the caller). Prints `OK` lines like [`measure_script`] or
/// `GONE` when the commit is not in the repository.
pub fn bundle_commit_script(
    path: &str,
    bundle: &str,
    head: &str,
    base: &str,
    tmp_ref: &str,
) -> String {
    let p = sh_quote(path);
    let b = sh_quote(bundle);
    let (h, ba) = (sh_quote(head), sh_quote(base));
    let r = sh_quote(tmp_ref);
    format!(
        "set -e; cd {p}; head={h}; base={ba}; \
         git cat-file -e \"$head^{{commit}}\" 2>/dev/null || {{ echo GONE; exit 5; }}; \
         git -c core.attributesFile=/dev/null -c attr.tree={EMPTY_TREE_SHA1} diff {STAT_FLAGS} --shortstat \"$base\" \"$head\" | sed 's/^/STAT /'; \
         if git cat-file -e \"$base^{{commit}}\" 2>/dev/null; then range=\"$base..$head\"; else range=\"$head\"; fi; \
         echo \"COMMITS $(git rev-list --count \"$range\")\"; \
         git update-ref {r} \"$head\"; \
         if git bundle create {b} {r} >/dev/null 2>&1; then rc=0; else rc=1; fi; \
         git update-ref -d {r}; \
         [ \"$rc\" = 0 ] || {{ echo NOBUNDLE; exit 6; }}; echo OK"
    )
}

/// Reads what [`measure_script`] printed.
fn parse_measured(
    path: &str,
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> Result<Measured> {
    if stdout.contains("UNCOMMITTED") {
        return Err(Error::invalid(
            "the repository has uncommitted changes; commit them first",
        ));
    }
    if stdout.contains("MOVED") {
        return Err(Error::invalid(
            "the repository changed while it was being read; try again",
        ));
    }
    if exit_code != Some(0) {
        return Err(Error::invalid(format!(
            "could not read the repository at {path}: {}",
            stderr.trim()
        )));
    }
    let mut m = Measured::default();
    for l in stdout.lines() {
        if l.trim() == "REMOTE_SSH" {
            m.ssh_remote = true;
        } else if let Some(h) = l.strip_prefix("HEAD ") {
            m.head = h.trim().into();
        } else if let Some(h) = l.strip_prefix("BASE ") {
            m.base = h.trim().into();
        } else if let Some(s) = l.strip_prefix("STAT ") {
            m.stat = s.trim().into();
        } else if let Some(c) = l.strip_prefix("COMMITS ") {
            m.commits = c.trim().parse().unwrap_or(0);
        }
    }
    let (files, additions, deletions) = crate::persistent::push_card::parse_shortstat(&m.stat);
    m.files = files as usize;
    m.additions = additions;
    m.deletions = deletions;
    if m.head.is_empty() {
        return Err(Error::invalid("the repository has no commits"));
    }
    if !is_full_sha(&m.head) || !is_full_sha(&m.base) {
        return Err(Error::invalid(
            "could not read the repository's commit; nothing was sent",
        ));
    }
    Ok(m)
}

/// The most the host reads out of a Space for one push, and for how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PushLimits {
    /// The largest bundle read, in bytes.
    pub max_bytes: u64,
    /// The longest the read of it may take.
    pub download_timeout: Duration,
}

impl Default for PushLimits {
    fn default() -> Self {
        PushLimits {
            max_bytes: 256 * 1024 * 1024,
            download_timeout: Duration::from_secs(5 * 60),
        }
    }
}

impl PushLimits {
    /// The defaults, or this computer's own choice: `CUA_PUSH_BUNDLE_MAX_MB`
    /// and `CUA_PUSH_BUNDLE_TIMEOUT_SECS` (a setting of the computer, not a
    /// test hook: they only make the host read less).
    pub fn from_env() -> PushLimits {
        let mut l = PushLimits::default();
        let n = |name: &str| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|v| *v > 0)
        };
        if let Some(mb) = n("CUA_PUSH_BUNDLE_MAX_MB") {
            l.max_bytes = mb.saturating_mul(1024 * 1024);
        }
        if let Some(s) = n("CUA_PUSH_BUNDLE_TIMEOUT_SECS") {
            l.download_timeout = Duration::from_secs(s);
        }
        l
    }

    fn max_mb(&self) -> u64 {
        self.max_bytes.div_ceil(1024 * 1024)
    }
}

/// The card-visible words when the code is not what the person approved.
pub const CODE_CHANGED: &str = "the code changed after you approved: nothing was pushed";

/// Why an approved push did not go out.
#[derive(Debug)]
pub enum PushError {
    /// Stopped for a reason that is shown to the person in plain words
    /// (the code changed after the answer, the bundle is too big or too
    /// slow). Nothing was pushed.
    Stopped {
        /// What the card says, in plain words.
        card: String,
        /// What was found (for the audit log and the agent), when more.
        detail: String,
    },
    /// Anything else.
    Failed(Error),
}

impl From<Error> for PushError {
    fn from(e: Error) -> Self {
        PushError::Failed(e)
    }
}

impl From<std::io::Error> for PushError {
    fn from(e: std::io::Error) -> Self {
        PushError::Failed(e.into())
    }
}

impl std::fmt::Display for PushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PushError::Stopped { card, detail } if card == detail => f.write_str(card),
            PushError::Stopped { card, detail } => write!(f, "{card} ({detail})"),
            PushError::Failed(e) => write!(f, "{e}"),
        }
    }
}

impl From<PushError> for Error {
    fn from(e: PushError) -> Error {
        match e {
            PushError::Failed(e) => e,
            e @ PushError::Stopped { .. } => Error::invalid(e.to_string()),
        }
    }
}

/// `s` as a sentence for a card: first letter up, a full stop at the end.
pub fn sentence(s: &str) -> String {
    let mut c = s.trim().chars();
    let mut out: String = match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => return String::new(),
    };
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

fn changed(detail: impl std::fmt::Display) -> PushError {
    PushError::Stopped {
        card: CODE_CHANGED.into(),
        detail: detail.to_string(),
    }
}

fn plain(words: String) -> PushError {
    PushError::Stopped {
        card: words.clone(),
        detail: words,
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

/// A bundle that was checked against what the card showed, as a bare
/// repository that holds exactly its objects.
pub struct Verified {
    dir: tempfile::TempDir,
    /// The commit that was verified, in full: what a push sends.
    pub commit: String,
}

impl Verified {
    /// Runs git in the verified repository, with the same hardening as the
    /// check itself: for reading what the verified commits hold (a diff, a
    /// file), by a viewer that must show exactly what was checked. Text,
    /// trimmed.
    pub async fn git(&self, args: &[&str]) -> std::result::Result<String, String> {
        check_git(self.dir.path(), args).await
    }

    /// [`Verified::git`], the output as it is (a file's bytes).
    pub async fn git_bytes(&self, args: &[&str]) -> std::result::Result<Vec<u8>, String> {
        check_git_raw(self.dir.path(), args).await
    }
}

/// Runs git for the check on a bundle the agent made: no system or user
/// config, no hooks, no templates, no attributes, no external diff, no
/// submodule fetching, no prompts. The bundle only carries objects and ref
/// names; nothing of its repository's config, hooks or filters comes with it.
async fn check_git_raw(dir: &Path, args: &[&str]) -> std::result::Result<Vec<u8>, String> {
    let home = dir.join("home");
    let _ = std::fs::create_dir_all(&home);
    let mut c = tokio::process::Command::new("git");
    c.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "protocol.ext.allow=never",
        "-c",
        "fetch.recurseSubmodules=false",
        "-c",
        "transfer.fsckObjects=true",
        "-c",
        &format!("attr.tree={EMPTY_TREE_SHA1}"),
        "-c",
        "core.attributesFile=/dev/null",
    ])
    .args(args)
    .current_dir(dir)
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env("GIT_CONFIG_GLOBAL", "/dev/null")
    .env("HOME", &home)
    .env("XDG_CONFIG_HOME", &home)
    .env("GIT_TERMINAL_PROMPT", "0")
    .env("GIT_ATTR_SOURCE", EMPTY_TREE_SHA1)
    .env_remove("GIT_DIR")
    .env_remove("GIT_WORK_TREE")
    .env_remove("GIT_INDEX_FILE")
    .env_remove("GIT_OBJECT_DIRECTORY")
    .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
    .env_remove("GIT_EXTERNAL_DIFF")
    .env_remove("GIT_CONFIG_PARAMETERS")
    .env_remove("GIT_CONFIG_COUNT")
    .env_remove("GIT_SSH_COMMAND")
    .env_remove("GIT_SSH")
    .env_remove("GIT_PROXY_COMMAND")
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    let o = tokio::time::timeout(Duration::from_secs(120), c.output())
        .await
        .map_err(|_| "git timed out".to_string())?
        .map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(o.stdout)
    } else {
        Err(redact(&String::from_utf8_lossy(&o.stderr)))
    }
}

/// [`check_git_raw`], its output as trimmed text.
async fn check_git(dir: &Path, args: &[&str]) -> std::result::Result<String, String> {
    check_git_raw(dir, args)
        .await
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
}

/// Checks the downloaded bundle against what the card showed, on this
/// computer, before anything is pushed. In a bare repository made for it, with
/// nothing of the bundle's own config in play, it fetches the bundle and
/// requires that `HEAD` is exactly the commit the card named (a bundle that
/// needs history it does not carry fails here), that the card's base is in
/// its history, and that the diffstat and the number of commits, recomputed
/// by git, are the ones shown. Anything else is [`PushError::Stopped`] with
/// [`CODE_CHANGED`], and nothing is sent.
pub async fn verify_bundle(
    bundle: &Path,
    binding: &Binding,
    limits: &PushLimits,
) -> std::result::Result<Verified, PushError> {
    verify_bundle_ref(bundle, binding, limits, "HEAD").await
}

/// [`verify_bundle`] for a bundle whose commit is under the ref `head_ref`
/// (`HEAD` for a push; a temporary ref for a viewer that bundles a commit
/// other than the checkout's).
pub async fn verify_bundle_ref(
    bundle: &Path,
    binding: &Binding,
    limits: &PushLimits,
    head_ref: &str,
) -> std::result::Result<Verified, PushError> {
    if !is_full_sha(&binding.head) || !is_full_sha(&binding.base) {
        return Err(PushError::Failed(Error::invalid(
            "the commit the card named is not a full hash; nothing was sent",
        )));
    }
    let meta = std::fs::symlink_metadata(bundle)?;
    if !meta.file_type().is_file() {
        return Err(changed("the bundle is not a plain file"));
    }
    if meta.len() > limits.max_bytes {
        return Err(plain(format!(
            "the repository is larger than Cua sends in one push ({} MB): nothing was pushed",
            limits.max_mb()
        )));
    }
    let dir = tempfile::tempdir()?;
    let d = dir.path();
    check_git(d, &["init", "--bare", "-q", "--template=", "."])
        .await
        .map_err(|e| PushError::Failed(Error::invalid(format!("git failed: {e}"))))?;
    let b = bundle.to_string_lossy().to_string();
    let heads = check_git(d, &["bundle", "list-heads", &b])
        .await
        .map_err(|_| changed("it is not a git bundle"))?;
    let bundled = heads
        .lines()
        .filter_map(|l| l.split_once(' '))
        .find(|(_, name)| *name == head_ref)
        .map(|(sha, _)| sha.to_string());
    match bundled {
        None => return Err(changed(format!("it holds no {head_ref}"))),
        Some(sha) if sha != binding.head => {
            return Err(changed(format!(
                "it holds commit {}, not {}",
                short(&sha),
                short(&binding.head)
            )));
        }
        Some(_) => {}
    }
    check_git(
        d,
        &[
            "fetch",
            "-q",
            "--no-tags",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            &b,
            &format!("{head_ref}:refs/cua/verified"),
        ],
    )
    .await
    .map_err(|e| changed(format!("it does not carry the whole history: {e}")))?;
    let got = check_git(d, &["rev-parse", "--verify", "refs/cua/verified^{commit}"])
        .await
        .map_err(|_| changed("it holds no commit"))?;
    if got != binding.head {
        return Err(changed(format!(
            "it holds commit {}, not {}",
            short(&got),
            short(&binding.head)
        )));
    }
    // The base is the empty tree (a one-commit history) or a commit in the history.
    let empty = check_git(d, &["hash-object", "-t", "tree", "/dev/null"])
        .await
        .unwrap_or_default();
    let range = if binding.base == empty {
        binding.head.clone()
    } else {
        let is_commit = check_git(
            d,
            &["cat-file", "-e", &format!("{}^{{commit}}", binding.base)],
        )
        .await
        .is_ok();
        let ancestor = is_commit
            && check_git(
                d,
                &["merge-base", "--is-ancestor", &binding.base, &binding.head],
            )
            .await
            .is_ok();
        if !ancestor {
            return Err(changed("the commit it was compared with is not in it"));
        }
        format!("{}..{}", binding.base, binding.head)
    };
    let stat = check_git(
        d,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--diff-algorithm=myers",
            "--shortstat",
            &binding.base,
            &binding.head,
        ],
    )
    .await
    .map_err(|e| changed(format!("its changes cannot be counted: {e}")))?;
    let (files, additions, deletions) = crate::persistent::push_card::parse_shortstat(&stat);
    if (files, additions, deletions) != (binding.files, binding.additions, binding.deletions) {
        return Err(changed(format!(
            "its changes are {files} files, +{additions} -{deletions}, not the {} files, +{} -{} shown",
            binding.files, binding.additions, binding.deletions
        )));
    }
    let count = check_git(d, &["rev-list", "--count", &range])
        .await
        .map_err(|e| changed(format!("its commits cannot be counted: {e}")))?;
    if count.parse::<u64>().ok() != Some(binding.commits) {
        return Err(changed(format!(
            "it carries {count} commits, not the {} shown",
            binding.commits
        )));
    }
    Ok(Verified {
        dir,
        commit: binding.head.clone(),
    })
}

/// Reads the bundle at `bundle` (a path in the Space) to a folder on this
/// computer, at most `limits.max_bytes` and for at most
/// `limits.download_timeout`, and removes it from the Space. A file over the
/// size is cut off at the size and refused, never read whole.
pub async fn fetch_bundle(
    space: &cua_spaces::Space,
    bundle: &str,
    limits: &PushLimits,
) -> std::result::Result<(tempfile::TempDir, std::path::PathBuf), PushError> {
    let tmp = tempfile::tempdir()?;
    let file = tmp.path().join("push.bundle");
    let client = space.spacesd()?;
    let opts = cua_spacesd_client::DownloadOptions {
        // One byte over the limit: enough to know it is too big.
        length: limits.max_bytes.saturating_add(1),
        ..Default::default()
    };
    let got = tokio::time::timeout(
        limits.download_timeout,
        client.download_to_file(bundle, &file, opts),
    )
    .await;
    let _ = space
        .bash(
            &format!("rm -f {}", sh_quote(bundle)),
            Duration::from_secs(20),
        )
        .await;
    match got {
        Err(_) => Err(plain(
            "reading the repository out of the Space took too long: nothing was pushed".into(),
        )),
        Ok(Err(e)) => Err(PushError::Failed(Error::invalid(format!(
            "could not read the repository out of the Space: {e}"
        )))),
        Ok(Ok(r)) if r.size > limits.max_bytes => Err(plain(format!(
            "the repository is larger than Cua sends in one push ({} MB): nothing was pushed",
            limits.max_mb()
        ))),
        Ok(Ok(_)) => Ok((tmp, file)),
    }
}

/// Verifies the bundle at `bundle` (a host file) against `binding`
/// ([`verify_bundle`]), then pushes the verified commit, by its hash, to
/// `req.branch` of `req.repo`, and returns the pull request's URL when one
/// was opened. The sign-in is read now, for this push alone
/// (`GithubConnection::token_now`), and dropped at the end.
pub async fn push_bundle(
    conn: &GithubConnection,
    bundle: &Path,
    req: &PushRequest,
    binding: &Binding,
    limits: &PushLimits,
) -> std::result::Result<Pushed, PushError> {
    let Some(method) = conn.method(req.ssh_remote) else {
        return Err(conn.refusal().into());
    };
    // Before a token is read: the code is what the card showed, or nothing happens.
    let verified = verify_bundle(bundle, binding, limits).await?;
    let token = if method == Method::Token {
        conn.token_now().await?
    } else {
        None
    };
    let scrub = |s: String| match &token {
        Some(t) => t.scrub(&s),
        None => s,
    };
    let tmp = tempfile::tempdir()?;
    // A deploy key is used only when there is no token (a token also opens the pull request).
    let deploy = method == Method::DeployKey && !conn.is_local();
    let own = method == Method::GitOwn;
    let mut ssh_command = None;
    if deploy {
        let key = tmp.path().join("deploy-key");
        let mut pem = conn.deploy_key.clone().unwrap_or_default();
        if !pem.ends_with('\n') {
            pem.push('\n');
        }
        std::fs::write(&key, pem)?;
        // ssh refuses a key others can read (the folder is private to this push already on Windows).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600))?;
        }
        let known = tmp.path().join("known_hosts");
        std::fs::write(&known, GITHUB_HOST_KEY)?;
        ssh_command = Some(format!(
            "ssh -i {} -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile={}",
            key.display(),
            known.display()
        ));
    } else if own && req.ssh_remote {
        // This computer's own SSH key or agent; never asks a question.
        ssh_command = Some("ssh -o BatchMode=yes -o StrictHostKeyChecking=yes".to_string());
    }
    let use_ssh = deploy || (own && req.ssh_remote);
    let git = |args: &[&str], push: bool| {
        let mut c = tokio::process::Command::new("git");
        // The repository is the verified one; nothing of it runs: no hooks.
        c.args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "push.recurseSubmodules=no",
        ])
        .args(args)
        .current_dir(verified.dir.path())
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
        if !own {
            // Git's own sign-in lives partly in the system config; only that source reads it.
            c.env("GIT_CONFIG_NOSYSTEM", "1");
        }
        if push && let Some(t) = token.as_ref().filter(|_| !use_ssh) {
            // The credential reaches git through its environment, never
            // its argument list.
            c.env("GIT_CONFIG_COUNT", "1")
                .env("GIT_CONFIG_KEY_0", "http.extraheader")
                .env("GIT_CONFIG_VALUE_0", GithubConnection::basic(t));
        }
        if push && let Some(cmd) = &ssh_command {
            // Names the key file, not the key.
            c.env("GIT_SSH_COMMAND", cmd);
        }
        c
    };
    let run = |mut c: tokio::process::Command, own: bool| {
        let scrub = &scrub;
        async move {
            let o = tokio::time::timeout(Duration::from_secs(180), c.output())
                .await
                .map_err(|_| Error::invalid("git timed out"))??;
            if o.status.success() {
                Ok(())
            } else {
                Err(Error::invalid(format!(
                    "{}git failed: {}",
                    if own {
                        "could not push with this computer's own git sign-in: "
                    } else {
                        ""
                    },
                    scrub(redact(&String::from_utf8_lossy(&o.stderr)))
                )))
            }
        }
    };
    let url = if use_ssh {
        format!("{}/{}.git", conn.ssh_base.trim_end_matches('/'), req.repo)
    } else {
        format!(
            "{}/{}.git",
            conn.remote_base.trim_end_matches('/'),
            req.repo
        )
    };
    // By the verified hash, to the branch that was named: not by any ref the bundle claims.
    let push = format!("{}:refs/heads/{}", verified.commit, req.branch);
    run(git(&["push", "-q", &url, &push], true), own).await?;
    let commit = verified.commit.clone();
    let can_pr = token.is_some();
    let compare_url = (!can_pr && !conn.is_local()).then(|| {
        format!(
            "https://{}/{}/compare/{}?expand=1",
            conn.host, req.repo, req.branch
        )
    });
    let Some(token) = token.filter(|_| req.open_pr) else {
        return Ok(Pushed {
            pull_request: None,
            compare_url,
            commit,
        });
    };
    Ok(Pushed {
        pull_request: Some(open_pr(conn, &token, req).await?),
        compare_url: None,
        commit,
    })
}

/// What came of an answered card.
#[derive(Debug)]
pub enum Released {
    /// A person said not now: nothing was fetched, read or sent.
    Declined(String),
    /// A person said yes and the push was made.
    Pushed(Pushed),
    /// A person said yes but the push was stopped, and nothing was sent: the
    /// code is not what the card showed, or it is too big or too slow to
    /// read. `card` is for the person, to show under the card.
    Stopped { card: String, detail: String },
}

/// Acts on a person's answer. Only `Decision::Approved` fetches the bundle
/// (`fetch` gives the folder that holds it, which must outlive the push, and
/// its path), checks it against `binding` and pushes it; a decline touches
/// nothing, and no sign-in is read.
pub async fn release<F, Fut>(
    decision: Decision,
    conn: &GithubConnection,
    req: &PushRequest,
    binding: &Binding,
    limits: &PushLimits,
    fetch: F,
) -> Result<Released>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<
            Output = std::result::Result<(tempfile::TempDir, std::path::PathBuf), PushError>,
        >,
{
    match decision {
        Decision::Declined(note) => Ok(Released::Declined(note)),
        Decision::Approved => {
            let done = async {
                let (_keep, file) = fetch().await?;
                push_bundle(conn, &file, req, binding, limits).await
            }
            .await;
            match done {
                Ok(p) => Ok(Released::Pushed(p)),
                Err(PushError::Stopped { card, detail }) => Ok(Released::Stopped { card, detail }),
                Err(PushError::Failed(e)) => Err(e),
            }
        }
    }
}

/// What a push came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pushed {
    /// The pull request the host opened (only with a token).
    pub pull_request: Option<String>,
    /// The link that opens one by hand (with a deploy key or git's own sign-in, which cannot).
    pub compare_url: Option<String>,
    /// The commit that was pushed, in full: the one the card named.
    pub commit: String,
}

/// Removes anything credential-shaped from git's message.
fn redact(s: &str) -> String {
    s.lines()
        .map(|l| {
            if l.to_ascii_lowercase().contains("authorization") {
                "(header hidden)".to_string()
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(600)
        .collect()
}

async fn open_pr(conn: &GithubConnection, token: &Token, req: &PushRequest) -> Result<String> {
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("cua-spaces")
        .build()
        .map_err(|e| Error::invalid(e.to_string()))?;
    let api = conn.api_base.trim_end_matches('/');
    let base = match &req.base {
        Some(b) => b.clone(),
        None => default_branch(&http, api, token, &req.repo).await,
    };
    let resp = http
        .post(format!("{api}/repos/{}/pulls", req.repo))
        .bearer_auth(token.expose())
        .json(&json!({"title": req.title, "head": req.branch, "base": base, "body": req.body}))
        .send()
        .await
        .map_err(|e| {
            Error::invalid(format!(
                "pushed, but the pull request failed: {}",
                token.scrub(&e.to_string())
            ))
        })?;
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(Error::invalid(format!(
            "pushed, but GitHub refused the pull request ({status}): {}",
            token.scrub(v["message"].as_str().unwrap_or(""))
        )));
    }
    Ok(v["html_url"].as_str().unwrap_or_default().to_string())
}

async fn default_branch(http: &reqwest::Client, api: &str, token: &Token, repo: &str) -> String {
    let r = http
        .get(format!("{api}/repos/{repo}"))
        .bearer_auth(token.expose())
        .send()
        .await;
    if let Ok(r) = r
        && r.status().is_success()
        && let Ok(v) = r.json::<Value>().await
        && let Some(b) = v["default_branch"].as_str()
    {
        return b.to_string();
    }
    "main".into()
}

/// The card's text and details, built from what the host measured. `account` is
/// the sign-in the push will use, in plain words (no token is read for it).
pub fn card(
    agent: &str,
    req: &PushRequest,
    m: &Measured,
    from: &str,
    account: &str,
    can_open_pr: bool,
) -> (String, String) {
    let what = if req.open_pr && can_open_pr {
        "push a branch and open a pull request"
    } else {
        "push a branch"
    };
    let at = if req.host == github_auth::DEFAULT_HOST || req.host.is_empty() {
        String::new()
    } else {
        format!(" at {}", req.host)
    };
    let text = format!("{agent} wants to {what} on {}{at}.", req.repo);
    let changed = if m.stat.is_empty() {
        format!("{} files changed", m.files)
    } else {
        m.stat.clone()
    };
    let details = format!(
        "Branch {} on {}{at} from commit {}. {changed}. Built in {from}. \
         Cua pushes it from this computer as {account}; the Space holds no GitHub access.{}",
        req.branch,
        req.repo,
        &m.head[..m.head.len().min(8)],
        if req.open_pr && !can_open_pr {
            " This sign-in can push a branch but not open the pull request: you get the link."
        } else {
            ""
        },
    );
    (text, details)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::Answer;

    fn answer(store: &ThreadStore, author: Author, seq: u64, approve: bool) {
        let mut d = Draft::new(author, EntryKind::Answer, "");
        d.answer = Some(Answer {
            approval: seq,
            approve,
            note: String::new(),
            ..Default::default()
        });
        store.append(d).unwrap();
    }

    #[tokio::test]
    async fn only_a_person_releases_an_export() {
        let d = tempfile::tempdir().unwrap();
        let store = ThreadStore::open(d.path(), "local:dev");
        let s2 = store.clone();
        let t = tokio::spawn(async move {
            ask_owner(&s2, REPO_PUSH, "push?", "details", Duration::from_secs(5)).await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        // An agent's answer and the daemon's answer do not count.
        answer(&store, Author::agent("claude"), 1, true);
        answer(&store, Author::system("host"), 1, true);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!t.is_finished(), "an agent answered its own export");
        answer(&store, Author::owner(), 1, false);
        let (seq, dec) = t.await.unwrap().unwrap();
        assert_eq!(seq, 1);
        assert!(matches!(dec, Decision::Declined(_)));
    }

    #[tokio::test]
    async fn no_answer_is_never_a_yes() {
        let d = tempfile::tempdir().unwrap();
        let store = ThreadStore::open(d.path(), "local:dev");
        let r = ask_owner(&store, REPO_PUSH, "push?", "", Duration::from_millis(300)).await;
        assert!(r.is_err());
    }

    #[test]
    fn the_github_key_never_goes_into_a_space() {
        // It is the host's own: a run cannot ask for it by name.
        assert!(cua_spaces::agents::HOST_ONLY_KEYS.contains(&GITHUB_KEY));
        let keys = cua_spaces::agents::keys::AgentKeys::unavailable("test");
        let e =
            cua_spaces::agents::env_from_host_with(&keys, &[GITHUB_KEY.to_string()]).unwrap_err();
        assert!(e.to_string().contains("never goes into a Space"), "{e}");
    }

    #[test]
    fn names_are_checked() {
        assert!(valid_repo("trycua/demo-failing-tests"));
        assert!(!valid_repo("a/b/c"));
        assert!(!valid_repo("a/../b"));
        assert!(!valid_repo("a b/c"));
        assert!(valid_branch("fix/tests-1"));
        assert!(!valid_branch("-x"));
        assert!(!valid_branch("a..b"));
        assert!(!valid_branch("a b"));
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn pushes_a_bundle_to_a_local_remote_without_a_token() {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remotes/acme/demo.git");
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "--bare", "-q", "."]);
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", "."]);
        std::fs::write(work.join("a.txt"), "hi").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "fix"]);
        let head = git(&work, &["rev-parse", "HEAD"]);
        let bundle = root.path().join("x.bundle");
        git(
            &work,
            &["bundle", "create", bundle.to_str().unwrap(), "HEAD"],
        );
        let conn = GithubConnection {
            token: None,
            remote_base: format!("file://{}", root.path().join("remotes").display()),
            api_base: String::new(),
            ..Default::default()
        };
        let req = PushRequest {
            repo: "acme/demo".into(),
            branch: "fix/it".into(),
            title: "t".into(),
            body: String::new(),
            open_pr: false,
            base: None,
            host: "github.com".into(),
            ssh_remote: false,
        };
        let pr = push_bundle(&conn, &bundle, &req, &binding_of(&work), &lim())
            .await
            .unwrap();
        assert!(pr.pull_request.is_none() && pr.compare_url.is_none());
        assert_eq!(git(&remote, &["rev-parse", "refs/heads/fix/it"]), head);
    }

    #[tokio::test]
    async fn no_connection_refuses_before_anything() {
        let conn = GithubConnection {
            token: None,
            host: "github.com".into(),
            remote_base: "https://github.com".into(),
            api_base: String::new(),
            ..Default::default()
        }
        .with_git_helper(false);
        assert!(!conn.ready(false));
        let req = PushRequest {
            repo: "a/b".into(),
            branch: "x".into(),
            title: String::new(),
            body: String::new(),
            open_pr: false,
            base: None,
            host: "github.com".into(),
            ssh_remote: false,
        };
        let e = push_bundle(&conn, Path::new("/nope"), &req, &dummy_binding(), &lim())
            .await
            .unwrap_err();
        assert!(
            e.to_string()
                .contains("No GitHub sign-in found: run gh auth login or paste a token"),
            "{e}"
        );
    }

    #[tokio::test]
    async fn opens_a_pull_request_through_the_api() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let s2 = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut c, _) = l.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = c.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if req.starts_with("GET") {
                    r#"{"default_branch":"trunk"}"#
                } else {
                    s2.lock().unwrap().push_str(&req);
                    r#"{"html_url":"https://github.com/acme/demo/pull/7"}"#
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = c.write_all(resp.as_bytes()).await;
            }
        });
        let conn = GithubConnection {
            token: Some("tok".into()),
            remote_base: "file:///x".into(),
            api_base: format!("http://127.0.0.1:{port}"),
            ..Default::default()
        };
        let req = PushRequest {
            repo: "acme/demo".into(),
            branch: "fix".into(),
            title: "Fix tests".into(),
            body: "why".into(),
            open_pr: true,
            base: None,
            host: "github.com".into(),
            ssh_remote: false,
        };
        let url = open_pr(&conn, &Token::new("tok"), &req).await.unwrap();
        assert_eq!(url, "https://github.com/acme/demo/pull/7");
        let sent = seen.lock().unwrap().clone();
        assert!(sent.contains(r#""base":"trunk""#), "{sent}");
        assert!(sent.contains(r#""head":"fix""#));
    }

    #[test]
    fn the_card_says_what_the_host_measured() {
        let (t, d) = card(
            "claude",
            &PushRequest {
                repo: "acme/demo".into(),
                branch: "fix".into(),
                title: String::new(),
                body: String::new(),
                open_pr: true,
                base: None,
                host: "github.com".into(),
                ssh_remote: false,
            },
            &Measured {
                head: "0123456789abcdef".into(),
                stat: "2 files changed, 3 insertions(+)".into(),
                files: 2,
                ssh_remote: false,
                ..Default::default()
            },
            "Linux Space",
            "ada, signed in with gh",
            true,
        );
        assert!(t.contains("open a pull request"));
        assert!(
            d.contains("as ada, signed in with gh; the Space holds no GitHub access"),
            "{d}"
        );
        assert!(d.contains("01234567") && d.contains("2 files changed"));
        assert!(!d.contains('\u{b7}'));
    }

    #[tokio::test]
    async fn a_deploy_key_pushes_a_branch_and_gives_the_link_for_the_pull_request() {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remotes/acme/demo.git");
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "--bare", "-q", "."]);
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", "."]);
        std::fs::write(work.join("a.txt"), "hi").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "fix"]);
        let head = git(&work, &["rev-parse", "HEAD"]);
        let bundle = root.path().join("x.bundle");
        git(
            &work,
            &["bundle", "create", bundle.to_str().unwrap(), "HEAD"],
        );
        let conn = GithubConnection {
            host: "github.com".into(),
            remote_base: "https://github.com".into(),
            // A local folder stands in for GitHub's SSH endpoint; git ignores the ssh command for it.
            ssh_base: format!("file://{}", root.path().join("remotes").display()),
            ..Default::default()
        }
        .with_deploy_key(
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----",
        );
        assert!(conn.ready(false) && !conn.can_open_pr(false));
        let req = PushRequest {
            repo: "acme/demo".into(),
            branch: "fix/it".into(),
            title: "t".into(),
            body: String::new(),
            open_pr: true,
            base: None,
            host: "github.com".into(),
            ssh_remote: false,
        };
        let done = push_bundle(&conn, &bundle, &req, &binding_of(&work), &lim())
            .await
            .unwrap();
        assert_eq!(git(&remote, &["rev-parse", "refs/heads/fix/it"]), head);
        assert!(done.pull_request.is_none());
        assert_eq!(
            done.compare_url.as_deref(),
            Some("https://github.com/acme/demo/compare/fix/it?expand=1")
        );
        // The card says what this connection can do.
        let m = Measured {
            head,
            stat: String::new(),
            files: 1,
            ssh_remote: false,
            ..Default::default()
        };
        let (text, details) = card(
            "claude",
            &req,
            &m,
            "Space",
            &conn.account(false),
            conn.can_open_pr(false),
        );
        assert!(
            text.contains("push a branch") && !text.contains("pull request"),
            "{text}"
        );
        assert!(details.contains("you get the link"));
    }

    #[test]
    fn the_deploy_key_is_host_only_too() {
        assert!(cua_spaces::agents::HOST_ONLY_KEYS.contains(&DEPLOY_KEY));
    }

    // ---- the sign-in chain behind the push

    fn plan(source: github_auth::Source, login: Option<&str>) -> Plan {
        Plan {
            host: "github.com".into(),
            source,
            login: login.map(String::from),
            env_name: None,
            account: github_auth::account_words(source, login, None, "github.com"),
            can_open_pr: true,
        }
    }

    /// A bare local "GitHub" and a bundle of one commit.
    fn remote_and_bundle() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        String,
    ) {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remotes/acme/demo.git");
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "--bare", "-q", "."]);
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", "."]);
        std::fs::write(work.join("a.txt"), "hi").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "fix"]);
        let head = git(&work, &["rev-parse", "HEAD"]);
        let bundle = root.path().join("x.bundle");
        git(
            &work,
            &["bundle", "create", bundle.to_str().unwrap(), "HEAD"],
        );
        (root, remote, bundle, head)
    }

    fn req(branch: &str) -> PushRequest {
        PushRequest {
            repo: "acme/demo".into(),
            branch: branch.into(),
            title: "t".into(),
            body: String::new(),
            open_pr: false,
            base: None,
            host: "github.com".into(),
            ssh_remote: false,
        }
    }

    fn counting(
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        token: &'static str,
    ) -> TokenFn {
        std::sync::Arc::new(move |host: &str| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(github_auth::Resolved {
                host: host.into(),
                source: github_auth::Source::Gh,
                login: Some("ada".into()),
                env_name: None,
                token: Token::new(token),
            })
        })
    }

    #[tokio::test]
    async fn nothing_is_read_or_pushed_before_a_person_answers_and_a_decline_sends_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (root, remote, bundle, head) = remote_and_bundle();
        let binding = binding_of(&root.path().join("work"));
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let conn = GithubConnection {
            host: "github.com".into(),
            remote_base: format!("file://{}", root.path().join("remotes").display()),
            ..Default::default()
        }
        .with_plan(plan(github_auth::Source::Gh, Some("ada")))
        .with_token_fn(counting(calls.clone(), "gho_SECRET_TOKEN"));
        // The card is built from the plan: it names the account and reads no token.
        assert_eq!(conn.account(false), "ada, signed in with gh");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let d = tempfile::tempdir().unwrap();
        let store = ThreadStore::open(d.path(), "local:dev");
        let s2 = store.clone();
        let ask = tokio::spawn(async move {
            ask_owner(&s2, REPO_PUSH, "push?", "details", Duration::from_secs(5)).await
        });
        tokio::time::sleep(Duration::from_millis(150)).await;
        // While the card is open: no sign-in read, nothing on the remote.
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(!remote.join("refs/heads/fix").exists());
        // An agent's yes is not an answer.
        answer(&store, Author::agent("claude"), 1, true);
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!ask.is_finished());
        answer(&store, Author::owner(), 1, false);
        let (_, dec) = ask.await.unwrap().unwrap();
        let fetched = std::sync::atomic::AtomicBool::new(false);
        let out = release(dec, &conn, &req("fix"), &binding, &lim(), || async {
            fetched.store(true, Ordering::SeqCst);
            Ok((tempfile::tempdir()?, bundle.clone()))
        })
        .await
        .unwrap();
        assert!(matches!(out, Released::Declined(_)));
        assert!(
            !fetched.load(Ordering::SeqCst),
            "a decline fetched the bundle"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "a decline read the sign-in"
        );
        assert!(!remote.join("refs/heads/fix").exists(), "a decline pushed");
        // A yes by a person pushes, and reads the sign-in once.
        let out = release(
            Decision::Approved,
            &conn,
            &req("fix"),
            &binding,
            &lim(),
            || async { Ok((tempfile::tempdir()?, bundle.clone())) },
        )
        .await
        .unwrap();
        assert!(matches!(out, Released::Pushed(_)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(git(&remote, &["rev-parse", "refs/heads/fix"]), head);
    }

    #[tokio::test]
    async fn a_host_switched_off_sends_nothing_even_with_a_token() {
        let (root, remote, bundle, _) = remote_and_bundle();
        let conn = GithubConnection {
            host: "github.com".into(),
            remote_base: format!("file://{}", root.path().join("remotes").display()),
            unavailable: Some(Unavailable::Off("github.com".into())),
            ..Default::default()
        }
        .with_token("ghp_TOKEN");
        assert!(conn.is_off() && !conn.ready(true));
        let e = push_bundle(
            &conn,
            &bundle,
            &req("fix"),
            &binding_of(&root.path().join("work")),
            &lim(),
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("turned off"), "{e}");
        assert!(!remote.join("refs/heads/fix").exists());
    }

    #[tokio::test]
    async fn the_token_is_never_in_an_error_a_debug_print_or_the_card() {
        let (root, _remote, bundle, _) = remote_and_bundle();
        let secret = "ghp_NEVER_SHOW_THIS_0123456789";
        // The remote does not exist: git fails, and says so without the token.
        let conn = GithubConnection {
            host: "github.com".into(),
            remote_base: "file:///nonexistent-remote-root".into(),
            ..Default::default()
        }
        .with_token(secret);
        let e = push_bundle(
            &conn,
            &bundle,
            &req("fix"),
            &binding_of(&root.path().join("work")),
            &lim(),
        )
        .await
        .unwrap_err();
        assert!(!e.to_string().contains(secret), "{e}");
        assert!(!format!("{conn:?}").contains(secret));
        let (t, d) = card(
            "claude",
            &req("fix"),
            &Measured {
                head: "0123456789".into(),
                stat: String::new(),
                files: 1,
                ssh_remote: false,
                ..Default::default()
            },
            "Space",
            &conn.account(false),
            conn.can_open_pr(false),
        );
        assert!(!t.contains(secret) && !d.contains(secret));
    }

    #[tokio::test]
    async fn git_own_sign_in_pushes_over_ssh_when_the_repository_has_an_ssh_remote() {
        let (root, remote, bundle, head) = remote_and_bundle();
        let conn = GithubConnection {
            host: "github.com".into(),
            remote_base: "https://github.com".into(),
            ssh_base: format!("file://{}", root.path().join("remotes").display()),
            ..Default::default()
        }
        .with_git_helper(false);
        // An https remote and no helper: nothing to push with.
        assert!(!conn.ready(false));
        // An ssh remote: git's own keys push the branch, and the link opens the pull request.
        assert_eq!(conn.method(true), Some(Method::GitOwn));
        assert!(!conn.can_open_pr(true));
        assert!(conn.account(true).contains("own git sign-in"));
        let mut r = req("fix/own");
        r.ssh_remote = true;
        r.open_pr = true;
        let done = push_bundle(
            &conn,
            &bundle,
            &r,
            &binding_of(&root.path().join("work")),
            &lim(),
        )
        .await
        .unwrap();
        assert_eq!(git(&remote, &["rev-parse", "refs/heads/fix/own"]), head);
        assert_eq!(
            done.compare_url.as_deref(),
            Some("https://github.com/acme/demo/compare/fix/own?expand=1")
        );
    }

    #[test]
    fn an_enterprise_host_changes_the_card_and_the_urls() {
        let mut r = req("fix");
        r.host = "ghe.example.com".into();
        let (t, d) = card(
            "claude",
            &r,
            &Measured {
                head: "0123456789".into(),
                stat: String::new(),
                files: 1,
                ssh_remote: false,
                ..Default::default()
            },
            "Space",
            "ada-corp, signed in with gh on ghe.example.com",
            true,
        );
        assert!(t.contains("on acme/demo at ghe.example.com"), "{t}");
        assert!(d.contains("ada-corp"), "{d}");
        assert!(github_auth::valid_host("ghe.example.com"));
        assert!(!github_auth::valid_host("evil.com/path"));
        assert!(!github_auth::valid_host("a b"));
    }

    /// The made-up card the web demo shows (`?demo=thread-push`) is what `card` writes for it.
    #[test]
    fn the_demo_card_is_what_the_host_writes() {
        let demo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../apps/cua-spaces-web/src/bridge/adapters/demo/thread.ts");
        let Ok(src) = std::fs::read_to_string(&demo) else {
            return;
        };
        let konst = |name: &str| -> String {
            let at = src.find(&format!("export const {name} =")).expect(name);
            let rest = &src[at..];
            let q1 = rest.find('"').unwrap();
            let q2 = rest[q1 + 1..].find('"').unwrap();
            rest[q1 + 1..q1 + 1 + q2].to_string()
        };
        let mut r = req("fix/retry-backoff");
        r.repo = "acme/app".into();
        r.open_pr = true;
        let (text, details) = card(
            "hermes",
            &r,
            &Measured {
                head: "0a1b2c3d4e5f60718293".into(),
                stat: "2 files changed, 12 insertions(+), 3 deletions(-)".into(),
                files: 2,
                ssh_remote: false,
                ..Default::default()
            },
            "release-checks",
            "ada, signed in with gh",
            true,
        );
        assert_eq!(text, konst("DEMO_PUSH_TEXT"));
        assert_eq!(details, konst("DEMO_PUSH_DETAILS"));
    }

    // ---- the approval is bound to the content

    fn lim() -> PushLimits {
        PushLimits::default()
    }

    fn dummy_binding() -> Binding {
        Binding {
            head: "a".repeat(40),
            base: "b".repeat(40),
            files: 0,
            additions: 0,
            deletions: 0,
            commits: 0,
        }
    }

    /// What the card would be bound to for the repository at `work`: its
    /// `HEAD`, the commit before it (or the empty tree), and git's own counts.
    fn binding_of(work: &Path) -> Binding {
        let head = git(work, &["rev-parse", "HEAD"]);
        let parent = std::process::Command::new("git")
            .args(["rev-parse", "--verify", "HEAD~1"])
            .current_dir(work)
            .output()
            .unwrap();
        let has_parent = parent.status.success();
        let base = if has_parent {
            String::from_utf8_lossy(&parent.stdout).trim().to_string()
        } else {
            git(work, &["hash-object", "-t", "tree", "/dev/null"])
        };
        let stat = git(
            work,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--diff-algorithm=myers",
                "--shortstat",
                &base,
                &head,
            ],
        );
        let (files, additions, deletions) = crate::persistent::push_card::parse_shortstat(&stat);
        let range = if has_parent {
            format!("{base}..{head}")
        } else {
            head.clone()
        };
        let commits = git(work, &["rev-list", "--count", &range]).parse().unwrap();
        Binding {
            head,
            base,
            files,
            additions,
            deletions,
            commits,
        }
    }

    fn local_conn(root: &Path) -> GithubConnection {
        GithubConnection {
            host: "github.com".into(),
            remote_base: format!("file://{}", root.join("remotes").display()),
            ..Default::default()
        }
        .with_plan(plan(github_auth::Source::Gh, Some("ada")))
        // A made-up sign-in: a test never runs the real `gh`.
        .with_token_fn(counting(Default::default(), "gho_LOCAL_TEST"))
    }

    /// A second repository with different code, and its bundle at `path`.
    fn evil_bundle(root: &Path, path: &Path) -> String {
        let evil = root.join("evil");
        std::fs::create_dir_all(&evil).unwrap();
        git(&evil, &["init", "-q", "-b", "main", "."]);
        std::fs::write(evil.join("a.txt"), "rm -rf everything").unwrap();
        git(&evil, &["add", "."]);
        git(&evil, &["commit", "-q", "-m", "fix"]);
        git(&evil, &["bundle", "create", path.to_str().unwrap(), "HEAD"]);
        git(&evil, &["rev-parse", "HEAD"])
    }

    fn stopped(r: Result<Released>) -> (String, String) {
        match r.unwrap() {
            Released::Stopped { card, detail } => (card, detail),
            other => panic!("expected a stop, got {other:?}"),
        }
    }

    /// The attack: the card shows commit A; the agent, still running while
    /// the card waits, replaces the bundle with one of commit B. Nothing is
    /// pushed, no sign-in is read, and the card gets the plain sentence.
    #[tokio::test]
    async fn a_bundle_replaced_after_the_card_pushes_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (root, remote, shown, head) = remote_and_bundle();
        let binding = binding_of(&root.path().join("work"));
        let swapped = root.path().join("swapped.bundle");
        let evil_head = evil_bundle(root.path(), &swapped);
        assert_ne!(evil_head, head);
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let conn = local_conn(root.path()).with_token_fn(counting(calls.clone(), "gho_X"));
        // The bundle the host downloads is the swapped one.
        let out = release(
            Decision::Approved,
            &conn,
            &req("fix"),
            &binding,
            &lim(),
            || async { Ok((tempfile::tempdir()?, swapped.clone())) },
        )
        .await;
        let (card, detail) = stopped(out);
        assert_eq!(card, CODE_CHANGED);
        assert!(card.contains("the code changed after you approved: nothing was pushed"));
        assert!(
            detail.contains(&evil_head[..8]) && detail.contains(&head[..8]),
            "{detail}"
        );
        assert!(
            !remote.join("refs/heads/fix").exists(),
            "the swapped code was pushed"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "a token was read for code that was not shown"
        );
        // The bundle that was shown goes out, as that exact commit.
        let out = release(
            Decision::Approved,
            &conn,
            &req("fix"),
            &binding,
            &lim(),
            || async { Ok((tempfile::tempdir()?, shown.clone())) },
        )
        .await
        .unwrap();
        let Released::Pushed(done) = out else {
            panic!("not pushed")
        };
        assert_eq!(done.commit, head);
        assert_eq!(git(&remote, &["rev-parse", "refs/heads/fix"]), head);
    }

    #[tokio::test]
    async fn other_ways_to_make_the_code_differ_are_all_refused() {
        let (root, remote, bundle, head) = remote_and_bundle();
        let work = root.path().join("work");
        let good = binding_of(&work);
        let conn = local_conn(root.path());
        let try_it = |b: Binding, file: std::path::PathBuf| {
            let conn = conn.clone();
            async move {
                push_bundle(&conn, &file, &req("fix"), &b, &lim())
                    .await
                    .map(|p| p.commit)
            }
        };
        // The card named another commit.
        let mut b = good.clone();
        b.head = "0".repeat(40);
        assert!(matches!(
            try_it(b, bundle.clone()).await,
            Err(PushError::Stopped { .. })
        ));
        // The numbers shown are not what the commit changes.
        for tweak in [
            |b: &mut Binding| b.additions += 1,
            |b: &mut Binding| b.deletions += 1,
            |b: &mut Binding| b.files += 1,
            |b: &mut Binding| b.commits += 1,
        ] {
            let mut b = good.clone();
            tweak(&mut b);
            let e = try_it(b, bundle.clone()).await.unwrap_err();
            assert!(
                matches!(&e, PushError::Stopped { card, .. } if card == CODE_CHANGED),
                "{e}"
            );
        }
        // The base shown is not in the history.
        let mut b = good.clone();
        b.base = "1".repeat(40);
        assert!(matches!(
            try_it(b, bundle.clone()).await,
            Err(PushError::Stopped { .. })
        ));
        // Not a bundle at all, a missing file, a folder.
        let junk = root.path().join("junk.bundle");
        std::fs::write(&junk, "not a bundle").unwrap();
        assert!(matches!(
            try_it(good.clone(), junk).await,
            Err(PushError::Stopped { .. })
        ));
        assert!(matches!(
            try_it(good.clone(), root.path().to_path_buf()).await,
            Err(PushError::Stopped { .. })
        ));
        // A bundle of the right commit that leaves its history behind.
        std::fs::write(work.join("b.txt"), "two").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "two"]);
        let thin = root.path().join("thin.bundle");
        git(
            &work,
            &["bundle", "create", thin.to_str().unwrap(), "HEAD~1..HEAD"],
        );
        let two = binding_of(&work);
        let e = try_it(two.clone(), thin).await.unwrap_err();
        assert!(e.to_string().contains("whole history"), "{e}");
        // None of it reached the remote; the right pair does.
        assert!(!remote.join("refs/heads/fix").exists());
        let full = root.path().join("full.bundle");
        git(&work, &["bundle", "create", full.to_str().unwrap(), "HEAD"]);
        assert_eq!(try_it(two.clone(), full).await.unwrap(), two.head);
        assert_ne!(two.head, head);
    }

    #[tokio::test]
    async fn a_bundle_over_the_size_limit_is_refused_and_says_so() {
        let (root, remote, bundle, _) = remote_and_bundle();
        let binding = binding_of(&root.path().join("work"));
        let conn = local_conn(root.path());
        let small = PushLimits {
            max_bytes: 10,
            ..PushLimits::default()
        };
        let e = push_bundle(&conn, &bundle, &req("fix"), &binding, &small)
            .await
            .unwrap_err();
        let PushError::Stopped { card, .. } = &e else {
            panic!("{e}")
        };
        assert!(
            card.contains("larger than Cua sends") && card.contains("nothing was pushed"),
            "{card}"
        );
        assert!(!remote.join("refs/heads/fix").exists());
        // The limit is a setting with a default, in megabytes.
        assert_eq!(PushLimits::default().max_bytes, 256 * 1024 * 1024);
        assert_eq!(small.max_mb(), 1);
    }

    /// The bundle's repository can hold hooks, a filter driver, a textconv
    /// driver, `.gitattributes` that use them, and a `.gitmodules` with a
    /// command URL. None of it runs on the host.
    #[tokio::test]
    #[cfg(unix)]
    async fn config_hooks_filters_and_submodules_in_a_bundle_are_ignored() {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remotes/acme/demo.git");
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "--bare", "-q", "."]);
        let marker = root.path().join("MARKER");
        let touch = format!("touch {}", marker.display());
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", "."]);
        std::fs::write(work.join("a.txt"), "hi").unwrap();
        std::fs::write(work.join(".gitattributes"), "* filter=evil diff=evil\n").unwrap();
        std::fs::write(
            work.join(".gitmodules"),
            format!("[submodule \"sub\"]\n\tpath = sub\n\turl = \"ext::sh -c {touch}\"\n"),
        )
        .unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "fix"]);
        // Only now the repository gets its hostile config and hooks.
        for (k, v) in [
            ("filter.evil.clean", touch.as_str()),
            ("filter.evil.smudge", touch.as_str()),
            ("diff.evil.textconv", touch.as_str()),
            ("core.sshCommand", touch.as_str()),
            ("protocol.ext.allow", "always"),
        ] {
            git(&work, &["config", k, v]);
        }
        for hook in [
            "pre-push",
            "post-checkout",
            "reference-transaction",
            "pre-auto-gc",
        ] {
            let h = work.join(".git/hooks").join(hook);
            std::fs::write(&h, format!("#!/bin/sh\n{touch}\n")).unwrap();
            let mut perm = std::fs::metadata(&h).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perm, 0o755);
            std::fs::set_permissions(&h, perm).unwrap();
        }
        let bundle = root.path().join("x.bundle");
        git(
            &work,
            &["bundle", "create", bundle.to_str().unwrap(), "HEAD"],
        );
        let head = git(&work, &["rev-parse", "HEAD"]);
        let binding = binding_of(&work);
        assert_eq!(binding.head, head);
        let conn = local_conn(root.path());
        let done = push_bundle(&conn, &bundle, &req("fix"), &binding, &lim())
            .await
            .unwrap();
        assert_eq!(done.commit, head);
        assert_eq!(git(&remote, &["rev-parse", "refs/heads/fix"]), head);
        assert!(!marker.exists(), "something in the bundle ran on the host");
    }

    /// The push is by the verified hash to the named branch only: extra refs
    /// a bundle carries are not pushed.
    #[tokio::test]
    async fn only_the_named_branch_gets_the_shown_commit() {
        let (root, remote, _, head) = remote_and_bundle();
        let work = root.path().join("work");
        git(&work, &["checkout", "-q", "-b", "extra"]);
        std::fs::write(work.join("x.txt"), "x").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "extra"]);
        git(&work, &["checkout", "-q", "main"]);
        let bundle = root.path().join("two.bundle");
        git(
            &work,
            &[
                "bundle",
                "create",
                bundle.to_str().unwrap(),
                "HEAD",
                "extra",
            ],
        );
        let conn = local_conn(root.path());
        let done = push_bundle(&conn, &bundle, &req("fix"), &binding_of(&work), &lim())
            .await
            .unwrap();
        assert_eq!(done.commit, head);
        assert_eq!(git(&remote, &["rev-parse", "refs/heads/fix"]), head);
        let refs = git(&remote, &["for-each-ref", "--format=%(refname)"]);
        assert_eq!(refs, "refs/heads/fix", "{refs}");
    }

    /// The Space-side script and the host's check agree on a real repository
    /// (renames, a binary file, a branch of two commits).
    #[tokio::test]
    async fn the_space_side_numbers_are_the_ones_the_host_recomputes() {
        let root = tempfile::tempdir().unwrap();
        let work = root.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", "."]);
        std::fs::write(work.join("old.txt"), "x\n".repeat(30)).unwrap();
        std::fs::write(work.join("keep.txt"), "k\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-q", "-m", "base"]);
        std::fs::rename(work.join("old.txt"), work.join("new.txt")).unwrap();
        std::fs::write(work.join("keep.txt"), "k\nk2\n").unwrap();
        std::fs::write(work.join("logo.png"), [0u8, 159, 146, 150]).unwrap();
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-q", "-m", "work"]);
        // A user setting in the Space that would change a diffstat.
        git(&work, &["config", "diff.renames", "copies"]);
        git(&work, &["config", "diff.algorithm", "patience"]);
        let bundle = root.path().join("m.bundle");
        let o = std::process::Command::new("sh")
            .arg("-c")
            .arg(measure_script(
                work.to_str().unwrap(),
                bundle.to_str().unwrap(),
            ))
            .output()
            .unwrap();
        let m = parse_measured(
            "work",
            &String::from_utf8_lossy(&o.stdout),
            &String::from_utf8_lossy(&o.stderr),
            o.status.code(),
        )
        .unwrap();
        assert_eq!(m.head, git(&work, &["rev-parse", "HEAD"]));
        assert_eq!(m.base, git(&work, &["rev-parse", "HEAD~1"]));
        assert_eq!((m.files, m.commits), (4, 1), "{m:?}");
        let v = verify_bundle(&bundle, &Binding::from(&m), &lim())
            .await
            .unwrap();
        assert_eq!(v.commit, m.head);
        // A repository that moved while it was read is not measured.
        std::fs::write(work.join("dirty.txt"), "d").unwrap();
        let o = std::process::Command::new("sh")
            .arg("-c")
            .arg(measure_script(
                work.to_str().unwrap(),
                bundle.to_str().unwrap(),
            ))
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&o.stdout).contains("UNCOMMITTED"));
    }

    #[test]
    fn full_hashes_only() {
        assert!(is_full_sha(&"a".repeat(40)) && is_full_sha(&"A".repeat(64)));
        assert!(!is_full_sha("0123456789") && !is_full_sha(&"g".repeat(40)));
        assert!(!is_full_sha(""));
    }

    #[test]
    fn a_release_build_refuses_the_redirect_variables_and_a_dev_build_reads_them() {
        let set = |n: &str| match n {
            "CUA_GITHUB_REMOTE_BASE" => Some("file:///somewhere".to_string()),
            _ => None,
        };
        let none = |_: &str| None;
        assert_eq!(
            Endpoints::from_vars("github.com", none).unwrap(),
            Endpoints::github("github.com")
        );
        let got = Endpoints::from_vars("github.com", set);
        #[cfg(feature = "dev-remotes")]
        assert_eq!(got.unwrap().remote_base, "file:///somewhere");
        #[cfg(not(feature = "dev-remotes"))]
        {
            let why = got.unwrap_err();
            assert!(
                why.contains("CUA_GITHUB_REMOTE_BASE") && why.contains("Nothing was sent"),
                "{why}"
            );
        }
        assert_eq!(
            Endpoints::github("ghe.example.com").remote_base,
            "https://ghe.example.com"
        );
    }

    #[tokio::test]
    async fn a_refused_redirect_sends_nothing_even_with_a_token() {
        let (root, remote, bundle, _) = remote_and_bundle();
        let mut conn = local_conn(root.path()).with_token("ghp_TOKEN");
        conn.refused = Some(
            "CUA_GITHUB_REMOTE_BASE would change where pushes go: unset it. Nothing was sent"
                .into(),
        );
        assert!(conn.blocked().is_some() && !conn.ready(true) && !conn.ready(false));
        let e = push_bundle(
            &conn,
            &bundle,
            &req("fix"),
            &binding_of(&root.path().join("work")),
            &lim(),
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("Nothing was sent"), "{e}");
        assert!(!remote.join("refs/heads/fix").exists());
    }
}
