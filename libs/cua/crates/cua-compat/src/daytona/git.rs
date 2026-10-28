//! Daytona toolbox: git (`/git/*`), over the `git` CLI in the guest.
//!
//! Request and response shapes come from the Apache-2.0 Daytona SDKs and
//! their generated toolbox clients (Python `daytona` 0.225.0:
//! `_sync/git.py`, `daytona_toolbox_api_client/{api/git_api.py,models/git_*.py}`;
//! TypeScript `@daytonaio/sdk` 0.225.0 `Git.js`); the implementation is ours.
//!
//! Credentials (`username`/`password`) reach git only through the environment
//! of one process and a throw-away `GIT_ASKPASS` helper that is removed when
//! the command ends. They are never put on a command line, never written to
//! `.git/config`, and are scrubbed from every message returned.
//! `dangerously_authenticate` is the one exception by design: it stores the
//! credential with git's own `credential.helper store`, as its name says.

use super::guestctx::{Guest, Ran};
use super::toolbox::body_json;
use super::{Daytona, SOURCE_DAEMON, coded, ok_json};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use http::{HeaderMap, Response, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route("/{id}/git/clone", post(clone))
        .route("/{id}/git/init", post(init))
        .route("/{id}/git/add", post(add))
        .route("/{id}/git/commit", post(commit))
        .route("/{id}/git/push", post(push))
        .route("/{id}/git/pull", post(pull))
        .route("/{id}/git/checkout", post(checkout))
        .route("/{id}/git/reset", post(reset))
        .route("/{id}/git/restore", post(restore))
        .route("/{id}/git/remotes", post(remote_add).get(remotes))
        .route("/{id}/git/credentials", post(credentials))
        .route("/{id}/git/config", post(set_config).get(get_config))
        .route("/{id}/git/config/user", post(configure_user))
        .route("/{id}/git/status", get(status))
        .route("/{id}/git/history", get(history))
        .route(
            "/{id}/git/branches",
            post(create_branch).get(branches).delete(delete_branch),
        )
        .with_state(d)
}

// ── pure helpers (unit and property tested) ─────────────────────────────

pub const CODE_AUTH: &str = "GIT_AUTH_FAILED";
pub const CODE_REPO_NOT_FOUND: &str = "GIT_REPO_NOT_FOUND";
pub const CODE_BRANCH_NOT_FOUND: &str = "GIT_BRANCH_NOT_FOUND";
pub const CODE_BRANCH_EXISTS: &str = "GIT_BRANCH_EXISTS";
pub const CODE_PUSH_REJECTED: &str = "GIT_PUSH_REJECTED";
pub const CODE_DIRTY: &str = "GIT_DIRTY_WORKTREE";
pub const CODE_MERGE_CONFLICT: &str = "GIT_MERGE_CONFLICT";
pub const CODE_TRANSPORT: &str = "GIT_TRANSPORT_FAILED";
pub const CODE_REMOTE_REJECTED: &str = "GIT_REMOTE_REJECTED";

/// Which toolbox operation failed (a few git messages mean different things
/// per operation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Clone,
    Checkout,
    Branch,
    Push,
    Pull,
    Other,
}

/// Maps a failed git command's stderr to the HTTP status and the daemon
/// error code the SDKs turn into `DaytonaGit*Error` (CODE_TO_ERROR in
/// `daytona/common/errors.py`). `None` code: a plain status error.
pub fn classify(op: Op, stderr: &str) -> (StatusCode, Option<&'static str>) {
    let e = stderr.to_ascii_lowercase();
    let has = |s: &str| e.contains(s);
    if has("authentication failed")
        || has("could not read username")
        || has("could not read password")
        || has("terminal prompts disabled")
        || has("invalid username or password")
        || has("error: 401")
        || has("error: 403")
        || has("permission denied (publickey")
        || has("http basic: access denied")
    {
        return (StatusCode::UNAUTHORIZED, Some(CODE_AUTH));
    }
    if has("not a git repository")
        || has("does not appear to be a git repository")
        || has("repository not found")
        || has("error: 404")
        || (has("repository '") && (has("not found") || has("does not exist")))
    {
        return (StatusCode::NOT_FOUND, Some(CODE_REPO_NOT_FOUND));
    }
    if has("remote rejected") || has("pre-receive hook declined") || has("hook declined") {
        return (StatusCode::UNPROCESSABLE_ENTITY, Some(CODE_REMOTE_REJECTED));
    }
    if has("[rejected]")
        || has("non-fast-forward")
        || has("fetch first")
        || has("failed to push some refs")
        || has("tip of your current branch is behind")
    {
        return (StatusCode::CONFLICT, Some(CODE_PUSH_REJECTED));
    }
    if has("conflict (")
        || has("automatic merge failed")
        || has("merge conflict")
        || has("fix conflicts")
        || has("you have unmerged paths")
    {
        return (StatusCode::CONFLICT, Some(CODE_MERGE_CONFLICT));
    }
    if has("would be overwritten by")
        || has("please commit your changes or stash")
        || has("your local changes to the following files")
        || has("local changes would be overwritten")
    {
        return (StatusCode::CONFLICT, Some(CODE_DIRTY));
    }
    if has("already exists") && !has("remote ") {
        let branchy = matches!(op, Op::Branch | Op::Checkout) && has("branch");
        if branchy || (op == Op::Branch) {
            return (StatusCode::CONFLICT, Some(CODE_BRANCH_EXISTS));
        }
    }
    if has("remote branch") && has("not found")
        || has("couldn't find remote ref")
        || has("could not find remote branch")
        || (op == Op::Checkout && has("pathspec") && has("did not match"))
        || (op == Op::Branch && has("branch") && has("not found"))
        || has("invalid reference")
    {
        return (StatusCode::NOT_FOUND, Some(CODE_BRANCH_NOT_FOUND));
    }
    if has("could not resolve host")
        || has("unable to access")
        || has("connection refused")
        || has("connection timed out")
        || has("failed to connect")
        || has("network is unreachable")
        || has("ssl ")
        || has("tls ")
        || has("early eof")
        || has("unexpected disconnect")
        || has("the remote end hung up")
        || has("could not read from remote repository")
    {
        return (StatusCode::BAD_GATEWAY, Some(CODE_TRANSPORT));
    }
    (StatusCode::BAD_REQUEST, None)
}

/// Removes `user:password@` (and a bare token) from every URL in `s`, and
/// any of `secrets` wherever they appear.
pub fn scrub(s: &str, secrets: &[&str]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("://") {
        out.push_str(&rest[..i + 3]);
        rest = &rest[i + 3..];
        let end = rest
            .find(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '/')
            .unwrap_or(rest.len());
        let authority = &rest[..end];
        match authority.rfind('@') {
            Some(at) => out.push_str(&authority[at + 1..]),
            None => out.push_str(authority),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    for sec in secrets.iter().filter(|s| s.len() >= 3) {
        out = out.replace(sec, "***");
    }
    out
}

/// `status` entry as the toolbox returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    pub name: String,
    pub extra: String,
    pub staging: &'static str,
    pub worktree: &'static str,
}

/// Parsed `git status --porcelain=v1 -z --branch`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusOut {
    pub branch: String,
    pub detached: bool,
    pub upstream: String,
    pub gone: bool,
    pub ahead: u32,
    pub behind: u32,
    pub files: Vec<FileStat>,
}

fn state_name(c: char) -> Option<&'static str> {
    Some(match c {
        ' ' => "Unmodified",
        'M' | 'T' => "Modified",
        'A' => "Added",
        'D' => "Deleted",
        'R' => "Renamed",
        'C' => "Copied",
        'U' => "Updated but unmerged",
        '?' => "Untracked",
        _ => return None,
    })
}

fn parse_branch_header(h: &str, out: &mut StatusOut) {
    if let Some(b) = h
        .strip_prefix("No commits yet on ")
        .or_else(|| h.strip_prefix("Initial commit on "))
    {
        out.branch = b.to_string();
        return;
    }
    if h.starts_with("HEAD (no branch)") {
        out.detached = true;
        out.branch = "HEAD".into();
        return;
    }
    let (names, track) = match h.find(" [") {
        Some(i) if h.ends_with(']') => (&h[..i], Some(&h[i + 2..h.len() - 1])),
        _ => (h, None),
    };
    match names.split_once("...") {
        Some((b, u)) => {
            out.branch = b.to_string();
            out.upstream = u.to_string();
        }
        None => out.branch = names.to_string(),
    }
    for part in track.unwrap_or("").split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            out.ahead = n.trim().parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            out.behind = n.trim().parse().unwrap_or(0);
        } else if part == "gone" {
            out.gone = true;
        }
    }
}

/// Parses `git status --porcelain=v1 -z --branch` output. Never panics.
pub fn parse_status(out: &[u8]) -> StatusOut {
    let text = String::from_utf8_lossy(out);
    let mut toks = text.split('\0');
    let mut st = StatusOut::default();
    while let Some(t) = toks.next() {
        if t.is_empty() {
            continue;
        }
        if let Some(h) = t.strip_prefix("## ") {
            parse_branch_header(h, &mut st);
            continue;
        }
        let mut chars = t.chars();
        let (Some(x), Some(y), Some(' ')) = (chars.next(), chars.next(), chars.next()) else {
            continue;
        };
        let name = chars.as_str().to_string();
        let mut extra = String::new();
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            extra = toks.next().unwrap_or("").to_string();
        }
        if x == '!' && y == '!' {
            continue;
        }
        let unmerged = x == 'U' || y == 'U' || (x == 'D' && y == 'D') || (x == 'A' && y == 'A');
        let (sx, sy) = if unmerged {
            ("Updated but unmerged", "Updated but unmerged")
        } else {
            match (state_name(x), state_name(y)) {
                (Some(a), Some(b)) => (a, b),
                _ => continue,
            }
        };
        st.files.push(FileStat {
            name,
            extra,
            staging: sx,
            worktree: sy,
        });
    }
    st
}

/// One `GitCommitInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub hash: String,
    pub author: String,
    pub email: String,
    pub timestamp: String,
    pub message: String,
}

pub const LOG_FORMAT: &str = "%H%x1f%an%x1f%ae%x1f%aI%x1f%B%x1e";

/// Parses `git log --format=LOG_FORMAT`.
pub fn parse_log(out: &str) -> Vec<Entry> {
    out.split('\x1e')
        .filter_map(|rec| {
            let rec = rec.trim_start_matches(['\n', '\r']);
            if rec.trim().is_empty() {
                return None;
            }
            let mut f = rec.splitn(5, '\x1f');
            Some(Entry {
                hash: f.next()?.to_string(),
                author: f.next()?.to_string(),
                email: f.next()?.to_string(),
                timestamp: f.next()?.to_string(),
                message: f.next()?.trim_end().to_string(),
            })
        })
        .collect()
}

/// Parses `git config --local -z --get-regexp '^remote\..+\.url$'`.
pub fn parse_remotes(out: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(out);
    let mut seen: Vec<(String, String)> = Vec::new();
    for rec in text.split('\0').filter(|r| !r.is_empty()) {
        let (key, url) = rec.split_once('\n').unwrap_or((rec, ""));
        let Some(name) = key
            .strip_prefix("remote.")
            .and_then(|k| k.strip_suffix(".url"))
        else {
            continue;
        };
        if !seen.iter().any(|(n, _)| n == name) {
            seen.push((name.to_string(), url.to_string()));
        }
    }
    seen
}

/// A ref or remote name that is safe to put on a command line: not empty,
/// not an option, no control characters.
pub fn safe_name(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('-') && !s.chars().any(|c| c.is_control() || c == ' ')
}

/// A path or pathspec: not empty, no NUL.
fn safe_arg(s: &str) -> bool {
    !s.is_empty() && !s.contains('\0')
}

/// Reset modes git accepts.
pub fn reset_mode(m: Option<&str>) -> Option<&'static str> {
    match m.unwrap_or("mixed") {
        "" | "mixed" => Some("--mixed"),
        "soft" => Some("--soft"),
        "hard" => Some("--hard"),
        "merge" => Some("--merge"),
        "keep" => Some("--keep"),
        _ => None,
    }
}

/// `--global` / `--local` / `--system` for a config scope.
pub fn scope_flag(scope: Option<&str>) -> Option<&'static str> {
    match scope.unwrap_or("global") {
        "" | "global" => Some("--global"),
        "local" => Some("--local"),
        "system" => Some("--system"),
        _ => None,
    }
}

// ── running git ─────────────────────────────────────────────────────────

/// The one shell script every git command runs through. All user data
/// arrives as positional arguments or environment variables, never as part
/// of the script text.
const RUN: &str = r#"d=$(mktemp -d 2>/dev/null) || { echo "fatal: cannot create a temporary directory" >&2; exit 97; }
trap 'rm -rf "$d"' EXIT
cd -- "$CUA_GIT_CWD" 2>/dev/null || { echo "fatal: not a git repository: $CUA_GIT_CWD: no such directory" >&2; exit 128; }
if [ -n "$CUA_GIT_PASS$CUA_GIT_USER" ]; then
  printf '#!/bin/sh\ncase "$1" in Username*) printf %%s "$CUA_GIT_USER";; *) printf %%s "$CUA_GIT_PASS";; esac\n' >"$d/ap"
  chmod 700 "$d/ap"
  export GIT_ASKPASS="$d/ap"
fi
export GIT_TERMINAL_PROMPT=0
if [ -z "$(git config user.name 2>/dev/null)" ]; then export GIT_AUTHOR_NAME="${GIT_AUTHOR_NAME:-daytona}" GIT_COMMITTER_NAME="${GIT_COMMITTER_NAME:-daytona}"; fi
if [ -z "$(git config user.email 2>/dev/null)" ]; then export GIT_AUTHOR_EMAIL="${GIT_AUTHOR_EMAIL:-daytona@localhost}" GIT_COMMITTER_EMAIL="${GIT_COMMITTER_EMAIL:-daytona@localhost}"; fi
"$@""#;

const INSTALL: &str = r#"command -v git >/dev/null 2>&1 && exit 0
if command -v apt-get >/dev/null 2>&1; then DEBIAN_FRONTEND=noninteractive apt-get install -y git >/dev/null 2>&1 || { apt-get update >/dev/null 2>&1 && DEBIAN_FRONTEND=noninteractive apt-get install -y git >/dev/null 2>&1; }
elif command -v apk >/dev/null 2>&1; then apk add --no-cache git >/dev/null 2>&1
elif command -v dnf >/dev/null 2>&1; then dnf install -y git >/dev/null 2>&1
elif command -v yum >/dev/null 2>&1; then yum install -y git >/dev/null 2>&1
fi
command -v git >/dev/null 2>&1"#;

/// One git invocation.
struct Git {
    cwd: String,
    args: Vec<String>,
    creds: Option<(String, String)>,
    insecure_tls: bool,
    env: Vec<(String, String)>,
    timeout: Duration,
}

impl Git {
    fn new(cwd: impl Into<String>, args: &[&str]) -> Self {
        Git {
            cwd: cwd.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            creds: None,
            insecure_tls: false,
            env: Vec::new(),
            timeout: Duration::from_secs(300),
        }
    }
    fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }
    fn creds(mut self, user: Option<&str>, pass: Option<&str>) -> Self {
        if user.is_some_and(|u| !u.is_empty()) || pass.is_some_and(|p| !p.is_empty()) {
            self.creds = Some((
                user.unwrap_or_default().to_string(),
                pass.unwrap_or_default().to_string(),
            ));
        }
        self
    }
}

fn secrets_of(c: &Option<(String, String)>) -> Vec<&str> {
    c.iter()
        .flat_map(|(u, p)| [u.as_str(), p.as_str()])
        .collect()
}

async fn ensure_git(d: &Daytona, g: &Guest) -> bool {
    let r = crate::guest::run_as(&g.c, &g.driver, "root", INSTALL, &[]).await;
    let _ = d;
    matches!(r, Ok(o) if o.success())
}

/// Runs `g` in the sandbox. A missing git is installed once.
async fn run(d: &Daytona, guest: &Guest, git: Git) -> Result<Ran, Response<Body>> {
    let mut argv: Vec<String> = vec!["/bin/sh".into(), "-c".into(), RUN.into(), "sh".into()];
    argv.push("git".into());
    if git.insecure_tls {
        argv.extend(["-c".into(), "http.sslVerify=false".into()]);
    }
    argv.extend(git.args.iter().cloned());
    let mut env = guest.env();
    env.insert("HOME".into(), guest.home.clone());
    env.insert("CUA_GIT_CWD".into(), git.cwd.clone());
    if let Some((u, p)) = &git.creds {
        env.insert("CUA_GIT_USER".into(), u.clone());
        env.insert("CUA_GIT_PASS".into(), p.clone());
    } else {
        env.insert("CUA_GIT_USER".into(), String::new());
        env.insert("CUA_GIT_PASS".into(), String::new());
    }
    env.extend(git.env.iter().cloned());
    let mut tried = false;
    loop {
        let ran = guest
            .run(d, argv.clone(), env.clone(), None, Some(git.timeout))
            .await?;
        let missing =
            ran.code == 127 && ran.stderr.contains("git") && ran.stderr.contains("not found");
        if missing && !tried {
            tried = true;
            if ensure_git(d, guest).await {
                continue;
            }
            return Err(plain(
                StatusCode::INTERNAL_SERVER_ERROR,
                "git is not installed in the sandbox and could not be installed",
            ));
        }
        return Ok(ran);
    }
}

fn plain(status: StatusCode, msg: impl Into<String>) -> Response<Body> {
    coded(status, SOURCE_DAEMON, None, msg)
}

fn failure(op: Op, ran: &Ran, secrets: &[&str]) -> Response<Body> {
    let raw = if ran.stderr.trim().is_empty() {
        &ran.stdout
    } else {
        &ran.stderr
    };
    let (status, code) = classify(op, raw);
    let msg = scrub(raw.trim(), secrets);
    let msg = if msg.is_empty() {
        format!("git exited with code {}", ran.code)
    } else {
        msg
    };
    coded(status, SOURCE_DAEMON, code, msg)
}

/// Runs and requires success.
async fn run_ok(d: &Daytona, g: &Guest, op: Op, git: Git) -> Result<Ran, Response<Body>> {
    let secrets: Vec<String> = secrets_of(&git.creds)
        .iter()
        .map(|s| s.to_string())
        .collect();
    let refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let ran = run(d, g, git).await?;
    if ran.code == 0 {
        Ok(ran)
    } else {
        Err(failure(op, &ran, &refs))
    }
}

fn empty_ok() -> Response<Body> {
    Response::builder()
        .status(StatusCode::OK)
        .body(Body::empty())
        .expect("response")
}

// ── request plumbing ────────────────────────────────────────────────────

async fn open(
    d: &Daytona,
    headers: &HeaderMap,
    id: &str,
    body: Body,
) -> Result<(Guest, Value), Response<Body>> {
    let g = Guest::open(d, headers, id).await?;
    let b = body_json(body).await?;
    Ok((g, b))
}

fn s<'a>(b: &'a Value, k: &str) -> Option<&'a str> {
    b.get(k).and_then(Value::as_str)
}

fn flag(b: &Value, k: &str) -> bool {
    b.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn repo(g: &Guest, b: &Value) -> Result<String, Response<Body>> {
    match s(b, "path").filter(|p| safe_arg(p)) {
        Some(p) => Ok(g.path(p)),
        None => Err(plain(StatusCode::BAD_REQUEST, "path is required")),
    }
}

fn need<'a>(b: &'a Value, k: &str) -> Result<&'a str, Response<Body>> {
    s(b, k)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| plain(StatusCode::BAD_REQUEST, format!("{k} is required")))
}

fn need_name<'a>(b: &'a Value, k: &str) -> Result<&'a str, Response<Body>> {
    let v = need(b, k)?;
    if safe_name(v) {
        Ok(v)
    } else {
        Err(plain(
            StatusCode::BAD_REQUEST,
            format!("invalid {k}: {v:?}"),
        ))
    }
}

fn opt_name<'a>(b: &'a Value, k: &str) -> Result<Option<&'a str>, Response<Body>> {
    match s(b, k).filter(|v| !v.is_empty()) {
        None => Ok(None),
        Some(v) if safe_name(v) => Ok(Some(v)),
        Some(v) => Err(plain(
            StatusCode::BAD_REQUEST,
            format!("invalid {k}: {v:?}"),
        )),
    }
}

fn files_of(b: &Value, required: bool) -> Result<Vec<String>, Response<Body>> {
    let files: Vec<String> = b
        .get("files")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if required && files.is_empty() {
        return Err(plain(StatusCode::BAD_REQUEST, "files is required"));
    }
    if files.iter().any(|f| !safe_arg(f)) {
        return Err(plain(StatusCode::BAD_REQUEST, "invalid file path"));
    }
    Ok(files)
}

// ── handlers ────────────────────────────────────────────────────────────

/// `POST /git/clone` `{url, path, branch?, commit_id?, username?, password?,
/// insecure_skip_tls?, depth?}`.
async fn clone(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let url = need(&b, "url")?;
        let path = repo(&g, &b)?;
        let branch = opt_name(&b, "branch")?;
        let commit = opt_name(&b, "commit_id")?;
        let mut git =
            Git::new(g.home.clone(), &["clone"]).creds(s(&b, "username"), s(&b, "password"));
        git.insecure_tls = flag(&b, "insecure_skip_tls");
        if let Some(depth) = b.get("depth").and_then(Value::as_u64).filter(|n| *n > 0) {
            git = git.arg("--depth").arg(depth.to_string());
        }
        if let Some(br) = branch {
            git = git.arg("--branch").arg(br);
        }
        git = git.arg("--").arg(url).arg(path.clone());
        run_ok(&d, &g, Op::Clone, git).await?;
        if let Some(c) = commit {
            let mut co = Git::new(path.clone(), &["checkout", "--detach"]).arg(c);
            co.timeout = Duration::from_secs(60);
            if let Err(e) = run_ok(&d, &g, Op::Checkout, co).await {
                // A bad commit leaves a clone at the default branch: undo it.
                let _ = g
                    .run(
                        &d,
                        vec!["rm".into(), "-rf".into(), "--".into(), path],
                        g.env(),
                        None,
                        Some(Duration::from_secs(60)),
                    )
                    .await;
                return Err(e);
            }
        }
        Ok::<_, Response<Body>>(())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

/// `POST /git/init` `{path, bare?, initial_branch?}`.
async fn init(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let path = repo(&g, &b)?;
        let branch = opt_name(&b, "initial_branch")?;
        let mut git = Git::new(g.home.clone(), &["init", "--quiet"]);
        if flag(&b, "bare") {
            git = git.arg("--bare");
        }
        if let Some(br) = branch {
            git = git.arg(format!("--initial-branch={br}"));
        }
        git = git.arg("--").arg(path);
        run_ok(&d, &g, Op::Other, git).await.map(|_| ())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

/// `POST /git/add` `{path, files}` (`.` adds everything).
async fn add(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let path = repo(&g, &b)?;
        let files = files_of(&b, true)?;
        let mut git = Git::new(path, &["add", "--"]);
        git.args.extend(files);
        run_ok(&d, &g, Op::Other, git).await.map(|_| ())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

/// `POST /git/commit` `{path, message, author, email, allow_empty?}` ->
/// `{hash}`.
async fn commit(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let path = repo(&g, &b)?;
        let message = s(&b, "message").filter(|m| !m.is_empty());
        let message =
            message.ok_or_else(|| plain(StatusCode::BAD_REQUEST, "message is required"))?;
        let author = need(&b, "author")?;
        let email = need(&b, "email")?;
        let mut git = Git::new(
            path.clone(),
            &["-c", "core.hooksPath=/dev/null", "commit", "--quiet"],
        );
        // Author and committer are the requester (Daytona signs both).
        git.env = vec![
            ("GIT_AUTHOR_NAME".into(), author.into()),
            ("GIT_COMMITTER_NAME".into(), author.into()),
            ("GIT_AUTHOR_EMAIL".into(), email.into()),
            ("GIT_COMMITTER_EMAIL".into(), email.into()),
        ];
        if flag(&b, "allow_empty") {
            git = git.arg("--allow-empty");
        }
        git = git.arg("-m").arg(message);
        run_ok(&d, &g, Op::Other, git).await?;
        let head = run_ok(&d, &g, Op::Other, Git::new(path, &["rev-parse", "HEAD"])).await?;
        Ok::<_, Response<Body>>(head.stdout.trim().to_string())
    };
    match go.await {
        Ok(hash) => ok_json(json!({"hash": hash})),
        Err(e) => e,
    }
}

/// `POST /git/push` `{path, username?, password?, branch?, remote?,
/// set_upstream?}`.
async fn push(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let path = repo(&g, &b)?;
        let remote = opt_name(&b, "remote")?.unwrap_or("origin");
        let branch = opt_name(&b, "branch")?;
        let mut git = Git::new(path, &["push"]).creds(s(&b, "username"), s(&b, "password"));
        if flag(&b, "set_upstream") {
            git = git.arg("--set-upstream");
        }
        // Without a branch the current one is pushed (to the same name).
        git = git.arg(remote).arg(branch.unwrap_or("HEAD"));
        run_ok(&d, &g, Op::Push, git).await.map(|_| ())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

/// `POST /git/pull` `{path, username?, password?, branch?, remote?}`.
async fn pull(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let path = repo(&g, &b)?;
        let remote = opt_name(&b, "remote")?;
        let branch = opt_name(&b, "branch")?;
        let mut git = Git::new(path, &["pull", "--no-rebase", "--no-edit"])
            .creds(s(&b, "username"), s(&b, "password"));
        if remote.is_some() || branch.is_some() {
            git = git.arg(remote.unwrap_or("origin"));
            if let Some(br) = branch {
                git = git.arg(br);
            }
        }
        run_ok(&d, &g, Op::Pull, git).await.map(|_| ())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

/// `POST /git/checkout` `{path, branch}`.
async fn checkout(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let path = repo(&g, &b)?;
        let branch = need_name(&b, "branch")?;
        run_ok(
            &d,
            &g,
            Op::Checkout,
            Git::new(path, &["checkout", "--quiet"]).arg(branch),
        )
        .await
        .map(|_| ())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

/// `POST /git/reset` `{path, mode?, target?, files?}`.
async fn reset(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let path = repo(&g, &b)?;
        let mode = reset_mode(s(&b, "mode")).ok_or_else(|| {
            plain(
                StatusCode::BAD_REQUEST,
                "mode must be one of soft, mixed, hard, merge, keep",
            )
        })?;
        let target = opt_name(&b, "target")?;
        let files = files_of(&b, false)?;
        let mut git = Git::new(path, &["reset", "--quiet"]);
        if files.is_empty() {
            git = git.arg(mode);
        }
        if let Some(t) = target {
            git = git.arg(t);
        }
        if !files.is_empty() {
            git = git.arg("--");
            git.args.extend(files);
        }
        run_ok(&d, &g, Op::Other, git).await.map(|_| ())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

/// `POST /git/restore` `{path, files, staged?, worktree?, source?}`.
async fn restore(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let path = repo(&g, &b)?;
        let files = files_of(&b, true)?;
        let source = opt_name(&b, "source")?;
        let mut git = Git::new(path, &["restore"]);
        if flag(&b, "staged") {
            git = git.arg("--staged");
        }
        if flag(&b, "worktree") {
            git = git.arg("--worktree");
        }
        if let Some(src) = source {
            git = git.arg(format!("--source={src}"));
        }
        git = git.arg("--");
        git.args.extend(files);
        run_ok(&d, &g, Op::Other, git).await.map(|_| ())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

/// `POST /git/remotes` `{path, name, url, fetch?, overwrite?}`.
async fn remote_add(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let path = repo(&g, &b)?;
        let name = need_name(&b, "name")?;
        let url = need(&b, "url")?;
        if flag(&b, "overwrite") {
            // Replace an existing remote of that name; absent is fine.
            let _ = run(
                &d,
                &g,
                Git::new(path.clone(), &["remote", "remove"]).arg(name),
            )
            .await?;
        }
        run_ok(
            &d,
            &g,
            Op::Other,
            Git::new(path.clone(), &["remote", "add", "--"])
                .arg(name)
                .arg(url),
        )
        .await?;
        if flag(&b, "fetch") {
            run_ok(
                &d,
                &g,
                Op::Pull,
                Git::new(path, &["fetch", "--quiet"]).arg(name),
            )
            .await?;
        }
        Ok::<_, Response<Body>>(())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

#[derive(Deserialize, Default)]
struct PathQuery {
    path: Option<String>,
    key: Option<String>,
    scope: Option<String>,
}

fn qrepo(g: &Guest, q: &PathQuery) -> Result<String, Response<Body>> {
    match q.path.as_deref().filter(|p| safe_arg(p)) {
        Some(p) => Ok(g.path(p)),
        None => Err(plain(StatusCode::BAD_REQUEST, "path is required")),
    }
}

/// `GET /git/remotes?path=` -> `{remotes: [{name, url}]}`.
async fn remotes(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<PathQuery>,
) -> Response<Body> {
    let g = match Guest::open(&d, &headers, &id).await {
        Ok(g) => g,
        Err(e) => return e,
    };
    let path = match qrepo(&g, &q) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let git = Git::new(
        path,
        &[
            "config",
            "--local",
            "-z",
            "--get-regexp",
            r"^remote\..+\.url$",
        ],
    );
    let ran = match run(&d, &g, git).await {
        Ok(r) => r,
        Err(e) => return e,
    };
    // Exit 1 with no output: no remote is configured.
    if ran.code != 0 && !(ran.code == 1 && ran.stderr.trim().is_empty()) {
        return failure(Op::Other, &ran, &[]);
    }
    let list: Vec<Value> = parse_remotes(ran.stdout.as_bytes())
        .into_iter()
        .map(|(name, url)| json!({"name": name, "url": url}))
        .collect();
    ok_json(json!({"remotes": list}))
}

/// `POST /git/credentials` `{username, password, host?, protocol?}`.
async fn credentials(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let user = need(&b, "username")?;
        let pass = need(&b, "password")?;
        let host = s(&b, "host")
            .filter(|h| !h.is_empty())
            .unwrap_or("github.com");
        let proto = s(&b, "protocol")
            .filter(|h| !h.is_empty())
            .unwrap_or("https");
        if host.contains(['\n', '\0'])
            || proto.contains(['\n', '\0'])
            || user.contains(['\n', '\0'])
            || pass.contains(['\n', '\0'])
        {
            return Err(plain(StatusCode::BAD_REQUEST, "invalid credential field"));
        }
        const SCRIPT: &str = r#"cd "$HOME" || exit 1
[ -n "$(git config --global credential.helper)" ] || git config --global credential.helper store || exit 1
printf 'protocol=%s\nhost=%s\nusername=%s\npassword=%s\n\n' "$CUA_GIT_PROTO" "$CUA_GIT_HOST" "$CUA_GIT_USER" "$CUA_GIT_PASS" | git credential approve"#;
        let mut env = g.env();
        env.insert("HOME".into(), g.home.clone());
        env.insert("CUA_GIT_PROTO".into(), proto.into());
        env.insert("CUA_GIT_HOST".into(), host.into());
        env.insert("CUA_GIT_USER".into(), user.into());
        env.insert("CUA_GIT_PASS".into(), pass.into());
        let ran = g
            .run(
                &d,
                vec!["/bin/sh".into(), "-c".into(), SCRIPT.into()],
                env,
                None,
                Some(Duration::from_secs(30)),
            )
            .await?;
        if ran.code != 0 {
            return Err(failure(Op::Other, &ran, &[user, pass]));
        }
        Ok::<_, Response<Body>>(())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

fn config_scope<'a>(
    g: &Guest,
    scope: Option<&str>,
    path: Option<&str>,
) -> Result<(&'a str, String), Response<Body>> {
    let flag = scope_flag(scope).ok_or_else(|| {
        plain(
            StatusCode::BAD_REQUEST,
            "scope must be one of global, local, system",
        )
    })?;
    if flag == "--local" {
        let p = path.filter(|p| safe_arg(p)).ok_or_else(|| {
            plain(
                StatusCode::BAD_REQUEST,
                "path is required when scope is local",
            )
        })?;
        return Ok((flag, g.path(p)));
    }
    Ok((flag, g.home.clone()))
}

/// `POST /git/config` `{key, value, scope?, path?}`.
async fn set_config(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let key = need_name(&b, "key")?;
        let value = b.get("value").and_then(Value::as_str).unwrap_or_default();
        let (flag, cwd) = config_scope(&g, s(&b, "scope"), s(&b, "path"))?;
        run_ok(
            &d,
            &g,
            Op::Other,
            Git::new(cwd, &["config", flag, "--", key, value]),
        )
        .await
        .map(|_| ())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

/// `GET /git/config?key=&scope=&path=` -> `{value}` (absent: `{}`).
async fn get_config(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<PathQuery>,
) -> Response<Body> {
    let g = match Guest::open(&d, &headers, &id).await {
        Ok(g) => g,
        Err(e) => return e,
    };
    let go = async {
        let key = q
            .key
            .as_deref()
            .filter(|k| safe_name(k))
            .ok_or_else(|| plain(StatusCode::BAD_REQUEST, "key is required"))?;
        let (flag, cwd) = config_scope(&g, q.scope.as_deref(), q.path.as_deref())?;
        let ran = run(&d, &g, Git::new(cwd, &["config", flag, "--get", "--", key])).await?;
        match ran.code {
            0 => Ok(Some(ran.stdout.trim_end_matches('\n').to_string())),
            1 if ran.stderr.trim().is_empty() => Ok(None),
            _ => Err(failure(Op::Other, &ran, &[])),
        }
    };
    match go.await {
        Ok(Some(v)) => ok_json(json!({"value": v})),
        Ok(None) => ok_json(json!({})),
        Err(e) => e,
    }
}

/// `POST /git/config/user` `{name, email, scope?, path?}`.
async fn configure_user(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let name = need(&b, "name")?;
        let email = need(&b, "email")?;
        let (flag, cwd) = config_scope(&g, s(&b, "scope"), s(&b, "path"))?;
        run_ok(
            &d,
            &g,
            Op::Other,
            Git::new(cwd.clone(), &["config", flag, "user.name", name]),
        )
        .await?;
        run_ok(
            &d,
            &g,
            Op::Other,
            Git::new(cwd, &["config", flag, "user.email", email]),
        )
        .await?;
        Ok::<_, Response<Body>>(())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

/// `GET /git/status?path=` -> `GitStatus`.
async fn status(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<PathQuery>,
) -> Response<Body> {
    let g = match Guest::open(&d, &headers, &id).await {
        Ok(g) => g,
        Err(e) => return e,
    };
    let go = async {
        let path = qrepo(&g, &q)?;
        let ran = run_ok(
            &d,
            &g,
            Op::Other,
            Git::new(
                path.clone(),
                &["status", "--porcelain=v1", "-z", "--branch"],
            ),
        )
        .await?;
        let st = parse_status(ran.stdout.as_bytes());
        // Published: a remote-tracking ref of this branch exists.
        let mut published = !st.upstream.is_empty() && !st.gone;
        if !published && !st.detached && !st.branch.is_empty() {
            let pat = format!("refs/remotes/*/{}", st.branch);
            if let Ok(r) = run(
                &d,
                &g,
                Git::new(path, &["for-each-ref", "--format=%(refname)"]).arg(pat),
            )
            .await
            {
                published = r.code == 0 && !r.stdout.trim().is_empty();
            }
        }
        Ok::<_, Response<Body>>((st, published))
    };
    match go.await {
        Ok((st, published)) => ok_json(json!({
            "currentBranch": st.branch,
            "detached": st.detached,
            "upstream": st.upstream,
            "ahead": st.ahead,
            "behind": st.behind,
            "branchPublished": published,
            "fileStatus": st.files.iter().map(|f| json!({
                "name": f.name, "extra": f.extra,
                "staging": f.staging, "worktree": f.worktree,
            })).collect::<Vec<_>>(),
        })),
        Err(e) => e,
    }
}

/// `GET /git/history?path=` -> `[GitCommitInfo]` (newest first; an empty
/// repository has none).
async fn history(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<PathQuery>,
) -> Response<Body> {
    let g = match Guest::open(&d, &headers, &id).await {
        Ok(g) => g,
        Err(e) => return e,
    };
    let path = match qrepo(&g, &q) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let ran = match run(
        &d,
        &g,
        Git::new(path, &["log"]).arg(format!("--format={LOG_FORMAT}")),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return e,
    };
    if ran.code != 0 {
        if ran.stderr.contains("does not have any commits yet") {
            return ok_json(json!([]));
        }
        return failure(Op::Other, &ran, &[]);
    }
    ok_json(Value::Array(
        parse_log(&ran.stdout)
            .into_iter()
            .map(|e| {
                json!({
                    "hash": e.hash, "author": e.author, "email": e.email,
                    "timestamp": e.timestamp, "message": e.message,
                })
            })
            .collect(),
    ))
}

/// `GET /git/branches?path=` -> `{branches, current}`.
async fn branches(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(q): Query<PathQuery>,
) -> Response<Body> {
    let g = match Guest::open(&d, &headers, &id).await {
        Ok(g) => g,
        Err(e) => return e,
    };
    let go = async {
        let path = qrepo(&g, &q)?;
        let list = run_ok(
            &d,
            &g,
            Op::Other,
            Git::new(
                path.clone(),
                &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
            ),
        )
        .await?;
        // Detached HEAD has no current branch; an unborn one still names it.
        let cur = run(
            &d,
            &g,
            Git::new(path, &["symbolic-ref", "--short", "-q", "HEAD"]),
        )
        .await?;
        let current = if cur.code == 0 {
            cur.stdout.trim().to_string()
        } else {
            String::new()
        };
        let names: Vec<String> = list
            .stdout
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        Ok::<_, Response<Body>>((names, current))
    };
    match go.await {
        Ok((names, current)) => ok_json(json!({"branches": names, "current": current})),
        Err(e) => e,
    }
}

/// `POST /git/branches` `{path, name}`: creates the branch at HEAD without
/// switching to it.
async fn create_branch(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let path = repo(&g, &b)?;
        let name = need_name(&b, "name")?;
        run_ok(
            &d,
            &g,
            Op::Branch,
            Git::new(path, &["branch", "--"]).arg(name),
        )
        .await
        .map(|_| ())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

/// `DELETE /git/branches` `{path, name}`.
async fn delete_branch(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response<Body> {
    let (g, b) = match open(&d, &headers, &id, body).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let go = async {
        let path = repo(&g, &b)?;
        let name = need_name(&b, "name")?;
        run_ok(
            &d,
            &g,
            Op::Branch,
            Git::new(path, &["branch", "-D", "--"]).arg(name),
        )
        .await
        .map(|_| ())
    };
    match go.await {
        Ok(()) => empty_ok(),
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn cfg() -> ProptestConfig {
        let cases = std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(64u32)
            .min(64);
        ProptestConfig::with_cases(cases)
    }

    #[test]
    fn classifies_the_error_families() {
        let c = |op, s| classify(op, s);
        assert_eq!(
            c(
                Op::Clone,
                "fatal: Authentication failed for 'https://h/r.git/'"
            ),
            (StatusCode::UNAUTHORIZED, Some(CODE_AUTH))
        );
        assert_eq!(
            c(
                Op::Clone,
                "fatal: could not read Username for 'https://h': terminal prompts disabled"
            ),
            (StatusCode::UNAUTHORIZED, Some(CODE_AUTH))
        );
        assert_eq!(
            c(
                Op::Clone,
                "fatal: '/nope' does not appear to be a git repository"
            ),
            (StatusCode::NOT_FOUND, Some(CODE_REPO_NOT_FOUND))
        );
        assert_eq!(
            c(
                Op::Other,
                "fatal: not a git repository (or any of the parent directories): .git"
            ),
            (StatusCode::NOT_FOUND, Some(CODE_REPO_NOT_FOUND))
        );
        assert_eq!(
            c(
                Op::Clone,
                "warning: Could not find remote branch zz to clone.\nfatal: Remote branch zz not found in upstream origin"
            ),
            (StatusCode::NOT_FOUND, Some(CODE_BRANCH_NOT_FOUND))
        );
        assert_eq!(
            c(
                Op::Checkout,
                "error: pathspec 'zz' did not match any file(s) known to git"
            ),
            (StatusCode::NOT_FOUND, Some(CODE_BRANCH_NOT_FOUND))
        );
        assert_eq!(
            c(
                Op::Other,
                "error: pathspec 'zz' did not match any file(s) known to git"
            ),
            (StatusCode::BAD_REQUEST, None)
        );
        assert_eq!(
            c(Op::Branch, "fatal: a branch named 'x' already exists"),
            (StatusCode::CONFLICT, Some(CODE_BRANCH_EXISTS))
        );
        assert_eq!(
            c(Op::Branch, "error: branch 'x' not found."),
            (StatusCode::NOT_FOUND, Some(CODE_BRANCH_NOT_FOUND))
        );
        assert_eq!(
            c(Op::Other, "error: remote origin already exists."),
            (StatusCode::BAD_REQUEST, None)
        );
        assert_eq!(
            c(
                Op::Push,
                " ! [rejected]        main -> main (non-fast-forward)\nerror: failed to push some refs"
            ),
            (StatusCode::CONFLICT, Some(CODE_PUSH_REJECTED))
        );
        assert_eq!(
            c(
                Op::Push,
                " ! [remote rejected] main -> main (pre-receive hook declined)\nerror: failed to push some refs"
            ),
            (StatusCode::UNPROCESSABLE_ENTITY, Some(CODE_REMOTE_REJECTED))
        );
        assert_eq!(
            c(
                Op::Pull,
                "CONFLICT (content): Merge conflict in a.txt\nAutomatic merge failed; fix conflicts"
            ),
            (StatusCode::CONFLICT, Some(CODE_MERGE_CONFLICT))
        );
        assert_eq!(
            c(
                Op::Checkout,
                "error: Your local changes to the following files would be overwritten by checkout:\n\ta.txt\nPlease commit your changes or stash them"
            ),
            (StatusCode::CONFLICT, Some(CODE_DIRTY))
        );
        assert_eq!(
            c(
                Op::Clone,
                "fatal: unable to access 'https://h/': Could not resolve host: h"
            ),
            (StatusCode::BAD_GATEWAY, Some(CODE_TRANSPORT))
        );
        assert_eq!(c(Op::Other, "boom"), (StatusCode::BAD_REQUEST, None));
    }

    #[test]
    fn scrubs_credentials() {
        assert_eq!(
            scrub(
                "fatal: unable to access 'https://bob:s3cret@host.example/r.git/': x",
                &["s3cret"]
            ),
            "fatal: unable to access 'https://host.example/r.git/': x"
        );
        assert_eq!(
            scrub("token ghp_abcdef here", &["ghp_abcdef"]),
            "token *** here"
        );
        assert_eq!(scrub("no urls", &[]), "no urls");
        assert_eq!(scrub("a://b", &[]), "a://b");
    }

    #[test]
    fn parses_status_headers() {
        let p = |s: &str| parse_status(s.as_bytes());
        let s = p("## main...origin/main [ahead 2, behind 1]\0");
        assert_eq!(
            (s.branch.as_str(), s.upstream.as_str(), s.ahead, s.behind),
            ("main", "origin/main", 2, 1)
        );
        let s = p("## feature/x...origin/feature/x [behind 3]\0");
        assert_eq!((s.branch.as_str(), s.ahead, s.behind), ("feature/x", 0, 3));
        let s = p("## main...origin/main [gone]\0");
        assert!(s.gone && s.upstream == "origin/main");
        let s = p("## No commits yet on trunk\0");
        assert_eq!((s.branch.as_str(), s.detached), ("trunk", false));
        let s = p("## HEAD (no branch)\0");
        assert!(s.detached);
        let s = p("## main\0");
        assert_eq!((s.branch.as_str(), s.upstream.as_str()), ("main", ""));
    }

    #[test]
    fn parses_status_entries() {
        let s = parse_status(b"## main\0 M a.txt\0A  b.txt\0?? c d.txt\0R  new.txt\0old.txt\0UU m.txt\0!! ign\0MD both\0");
        let f = |n: &str| s.files.iter().find(|f| f.name == n).cloned().unwrap();
        assert_eq!(
            (f("a.txt").staging, f("a.txt").worktree),
            ("Unmodified", "Modified")
        );
        assert_eq!(
            (f("b.txt").staging, f("b.txt").worktree),
            ("Added", "Unmodified")
        );
        assert_eq!(
            (f("c d.txt").staging, f("c d.txt").worktree),
            ("Untracked", "Untracked")
        );
        assert_eq!(
            (f("new.txt").staging, f("new.txt").extra.as_str()),
            ("Renamed", "old.txt")
        );
        assert_eq!(f("m.txt").worktree, "Updated but unmerged");
        assert_eq!(
            (f("both").staging, f("both").worktree),
            ("Modified", "Deleted")
        );
        assert!(!s.files.iter().any(|f| f.name == "ign"));
    }

    #[test]
    fn parses_log_and_remotes() {
        let out = "abc\x1fAnn\x1fa@x\x1f2026-01-01T00:00:00+00:00\x1fsubject\n\nbody\n\x1e\ndef\x1fBo\x1fb@x\x1f2026-01-02T00:00:00+00:00\x1fsecond\n\x1e\n";
        let l = parse_log(out);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].message, "subject\n\nbody");
        assert_eq!(l[1].hash, "def");
        assert!(parse_log("").is_empty());
        let r = parse_remotes(b"remote.origin.url\n/srv/a.git\0remote.up.stream.url\nhttps://h/r\0remote.origin.url\nignored\0");
        assert_eq!(
            r,
            vec![
                ("origin".into(), "/srv/a.git".into()),
                ("up.stream".into(), "https://h/r".into())
            ]
        );
    }

    #[test]
    fn validates_arguments() {
        assert!(safe_name("feature/x") && safe_name("v1.0"));
        assert!(
            !safe_name("--upload-pack=x")
                && !safe_name("")
                && !safe_name("a b")
                && !safe_name("a\nb")
        );
        assert_eq!(reset_mode(None), Some("--mixed"));
        assert_eq!(reset_mode(Some("hard")), Some("--hard"));
        assert_eq!(reset_mode(Some("bogus")), None);
        assert_eq!(scope_flag(None), Some("--global"));
        assert_eq!(scope_flag(Some("local")), Some("--local"));
        assert_eq!(scope_flag(Some("worktree")), None);
    }

    proptest! {
        #![proptest_config(cfg())]

        #[test]
        fn status_parser_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
            let _ = parse_status(&bytes);
            let _ = parse_remotes(&bytes);
            let _ = parse_log(&String::from_utf8_lossy(&bytes));
        }

        #[test]
        fn status_roundtrip(
            files in proptest::collection::vec(("[MADRCU? ][MADU? ]", "[a-z][a-z0-9 ./_-]{0,12}"), 0..8),
            branch in "[a-z][a-z0-9/_-]{0,10}",
            ahead in 0u32..50, behind in 0u32..50,
        ) {
            let mut out = format!("## {branch}...origin/{branch} [ahead {ahead}, behind {behind}]\0");
            let mut kept = 0;
            for (xy, name) in &files {
                if xy.contains('R') || xy.contains('C') { continue; }
                out.push_str(&format!("{xy} {name}\0"));
                kept += 1;
            }
            let s = parse_status(out.as_bytes());
            prop_assert_eq!(s.branch, branch);
            prop_assert_eq!((s.ahead, s.behind), (ahead, behind));
            prop_assert!(s.files.len() <= kept);
        }

        #[test]
        fn scrub_hides_the_password(user in "[a-z]{1,8}", pass in "[A-Za-z0-9]{6,16}", host in "[a-z]{1,8}\\.[a-z]{2,3}", pre in "[ -~]{0,20}") {
            let msg = format!("{pre} fatal: unable to access 'https://{user}:{pass}@{host}/r.git/'");
            let out = scrub(&msg, &[&pass]);
            prop_assert!(!out.contains(&pass) || pre.contains(&pass));
            let needle = format!("{}:{}@", user, pass);
            prop_assert!(!out.contains(&needle));
        }

        #[test]
        fn classify_is_total(s in ".{0,200}") {
            let (st, code) = classify(Op::Push, &s);
            prop_assert!(st.is_client_error() || st.is_server_error());
            prop_assert!(code.is_none_or(|c| c.starts_with("GIT_")));
        }

        #[test]
        fn names_with_option_prefix_are_refused(s in "-[ -~]{0,20}") {
            prop_assert!(!safe_name(&s));
        }
    }
}
