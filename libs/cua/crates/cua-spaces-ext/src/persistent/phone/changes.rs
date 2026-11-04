// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `space_changes`, `space_file_diff`, `space_file_preview`: what an agent
//! changed in a Space's git repository, for a phone: the files with their
//! `+` and `-` counts, one file's unified diff, and a small preview of a
//! file. Every answer is size-capped.
//!
//! Two sources:
//!
//! - **The Space's working state** (a `repo`, or the most recently used
//!   checkout under the Space's home): git is run in the Space, so the
//!   answer is what the Space says (`verified: false`). It is for looking,
//!   like the thread's own text: nothing is pushed on the strength of it.
//! - **A push approval's card** (`approval`): the commit pair the card was
//!   bound to. The commit is bundled in the Space, read out to this
//!   machine and checked by the same check that guards the push
//!   ([`host_tools::verify_bundle_ref`]), then the files, the diff and the
//!   previews are computed by git here from that checked content
//!   (`verified: true`). What the person reviews is what the host will
//!   push.
//!
//! Nothing here depends on how the Space runs: git runs through the
//! Space's shell, the same for a container, a VM or a cloud machine. A
//! Windows Space has no POSIX shell to run it in and says so.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{Value, json};

use super::super::Persistent;
use super::super::host_tools::{self as ht, Binding, PushLimits, Verified, sh_quote};
use super::super::push_card::{FileStat, parse_diffstat, parse_shortstat};
use cua_spaces::extension::Caller;
use cua_spaces::mcp::ToolOutcome;
use cua_spaces::{Error, Result, Space, Spaces};

/// A unified diff of one file is cut at this many bytes.
pub const DIFF_MAX: usize = 256 * 1024;
/// A preview is cut at this many bytes (the request may ask for less).
pub const PREVIEW_MAX: u64 = 1024 * 1024;
pub const PREVIEW_DEFAULT: u64 = 256 * 1024;
/// Files listed at most.
pub const FILES_MAX: usize = 200;
/// Lines of context around a change, at most.
pub const CONTEXT_MAX: u32 = 20;
/// How long a checked bundle is kept for the next call about it.
const VERIFIED_TTL: Duration = Duration::from_secs(600);
const VERIFIED_KEPT: usize = 4;
/// The largest bundle read for a viewer: less than a push's.
const BUNDLE_MAX_BYTES: u64 = 64 * 1024 * 1024;

fn valid_rev(r: &str) -> bool {
    !r.is_empty()
        && r.len() <= 200
        && !r.starts_with('-')
        && r.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./~^@{}-".contains(&b))
}

fn valid_file(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 1024
        && !p.starts_with('-')
        && !p.chars().any(|c| c == '\0' || c == '\n' || c == '\r')
}

fn valid_dir(p: &str) -> bool {
    !p.is_empty() && p.len() <= 1024 && !p.chars().any(|c| c == '\0' || c == '\n' || c == '\r')
}

fn get_str<'a>(a: &'a Value, k: &str) -> Option<&'a str> {
    a.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// What the request names.
struct Req {
    space: String,
    repo: Option<String>,
    base: Option<String>,
    approval: Option<u64>,
}

fn request(a: &Value, tool: &str) -> Result<Req> {
    let space = get_str(a, "space")
        .ok_or_else(|| Error::invalid(format!("{tool}: `space` is required")))?
        .to_string();
    let repo = get_str(a, "repo").map(str::to_string);
    if let Some(r) = &repo
        && !valid_dir(r)
    {
        return Err(Error::invalid(format!(
            "{tool}: `repo` is not a folder name"
        )));
    }
    let base = get_str(a, "base").map(str::to_string);
    if let Some(b) = &base
        && !valid_rev(b)
    {
        return Err(Error::invalid(format!(
            "{tool}: `base` is not a commit or branch name"
        )));
    }
    Ok(Req {
        space,
        repo,
        base,
        approval: a.get("approval").and_then(Value::as_u64),
    })
}

/// The Space, checked to run something git can be run in.
async fn space_for(
    spaces: &Spaces,
    p: &Persistent,
    caller: &Caller,
    name: &str,
) -> Result<(Space, String)> {
    // The same access check as reading the Space's thread.
    let t = p.thread(caller, name).await?;
    let id = t.store().space().to_string();
    if let Ok(rows) = spaces.list()
        && rows
            .iter()
            .any(|s| s.id == id && s.os.eq_ignore_ascii_case("windows"))
    {
        return Err(Error::invalid(
            "changes are read with git in the Space's shell: a Windows Space has none yet",
        ));
    }
    Ok((spaces.space(&id).await?, id))
}

/// `cd` into the repository: `repo`, or the checkout used most recently
/// under the home, then to its top.
fn enter(repo: Option<&str>) -> String {
    match repo {
        Some(r) => {
            let d = if let Some(rest) = r.strip_prefix("~/") {
                format!("\"$HOME\"/{}", sh_quote(rest))
            } else if r.starts_with('/') {
                sh_quote(r)
            } else {
                // A relative name is under the home.
                format!("\"$HOME\"/{}", sh_quote(r))
            };
            format!("cd {d} 2>/dev/null || {{ echo 'ERR no such folder'; exit 3; }}; ")
        }
        None => "d=$(ls -dt \"$HOME\"/.git \"$HOME\"/*/.git \"$HOME\"/*/*/.git 2>/dev/null | head -n 1); \
                 [ -n \"$d\" ] || { echo 'ERR no repository under the home'; exit 3; }; cd \"$(dirname \"$d\")\"; "
            .to_string(),
    }
}

const TOP: &str = "git rev-parse --is-inside-work-tree >/dev/null 2>&1 || { echo 'ERR not a git repository'; exit 4; }; \
                   top=$(git rev-parse --show-toplevel); cd \"$top\"; ";

/// The base commit: the one asked for, else where the branch left the
/// remote's default branch, else the checkout's `HEAD` (uncommitted work).
fn base_of(base: Option<&str>) -> String {
    match base {
        Some(b) => format!(
            "base=$(git rev-parse --verify -q {}) || {{ echo 'ERR unknown base'; exit 5; }}; ",
            sh_quote(&format!("{b}^{{commit}}"))
        ),
        None => "base=$(git merge-base \"$head\" origin/HEAD 2>/dev/null || echo \"$head\"); "
            .to_string(),
    }
}

const HEAD: &str =
    "head=$(git rev-parse --verify -q 'HEAD^{commit}' || git hash-object -t tree /dev/null); ";

/// The text a script printed, or the one-line reason it gave.
fn script_output(out: cua_spaces::exec::BashOutput) -> Result<String> {
    if let Some(line) = out.stdout.lines().find_map(|l| l.strip_prefix("ERR ")) {
        return Err(Error::invalid(line.trim().to_string()));
    }
    if out.exit_code != Some(0) && out.stdout.trim().is_empty() {
        return Err(Error::invalid(format!(
            "could not read the repository: {}",
            out.stderr.lines().next().unwrap_or("git failed").trim()
        )));
    }
    Ok(out.stdout)
}

/// A file's `+` and `-` counts and size, as the wire shows it.
fn file_json(f: &FileStat, size: Option<u64>) -> Value {
    let mut v = json!({
        "path": f.path,
        "status": f.status,
        "additions": f.additions.unwrap_or(0),
        "deletions": f.deletions.unwrap_or(0),
        "binary": f.binary,
        "size": size.unwrap_or(0),
    });
    if let Some(o) = &f.old_path {
        v["old_path"] = json!(o);
    }
    if let Some(m) = mime_of(&f.path).filter(|m| m.starts_with("image/")) {
        v["mime"] = json!(m);
    }
    v
}

/// A mime type from the name, for what a phone previews.
pub fn mime_of(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "txt" | "log" | "md" | "markdown" | "rs" | "py" | "ts" | "tsx" | "js" | "jsx" | "mjs"
        | "json" | "yaml" | "yml" | "toml" | "sh" | "swift" | "kt" | "go" | "java" | "c" | "h"
        | "cc" | "cpp" | "hpp" | "css" | "html" | "xml" | "svg" | "csv" | "sql" | "rb" | "php"
        | "ini" | "cfg" | "lock" => "text/plain",
        _ => return None,
    })
}

// ---------------------------------------------------------------- verified

type VerifiedKey = (String, u64);

/// A checked bundle and when it was made.
type Kept = (Instant, Arc<Verified>);

fn verified_cache() -> &'static Mutex<HashMap<VerifiedKey, Kept>> {
    static C: std::sync::OnceLock<Mutex<HashMap<VerifiedKey, Kept>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// What a push approval's card names, and where to find it.
struct Card {
    head: String,
    base: String,
    repo_path: Option<String>,
}

fn card_of(store: &crate::thread::ThreadStore, seq: u64) -> Result<Card> {
    let e = store
        .get(seq)?
        .ok_or_else(|| Error::NotFound(format!("approval #{seq} in this thread")))?;
    let card = e
        .approval
        .as_ref()
        .and_then(|a| a.card.as_ref())
        .ok_or_else(|| Error::invalid(format!("approval #{seq} has no changes to show")))?;
    let text = |k: &str| {
        card.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let (head, base) = (text("head"), text("baseSha"));
    if !ht::is_full_sha(&head) || !ht::is_full_sha(&base) {
        return Err(Error::invalid(format!(
            "approval #{seq}'s card names no commit to check"
        )));
    }
    Ok(Card {
        head,
        base,
        repo_path: Some(text("repoPath")).filter(|p| !p.is_empty()),
    })
}

/// The card's commit pair, bundled in the Space, read out and checked here.
async fn verified(
    space: &Space,
    space_id: &str,
    seq: u64,
    card: &Card,
    repo: Option<&str>,
) -> Result<Arc<Verified>> {
    let key = (space_id.to_string(), seq);
    {
        let mut c = verified_cache().lock().expect("verified cache");
        c.retain(|_, (at, _)| at.elapsed() < VERIFIED_TTL);
        if let Some((_, v)) = c.get(&key) {
            return Ok(v.clone());
        }
    }
    let path = repo
        .map(str::to_string)
        .or_else(|| card.repo_path.clone())
        .ok_or_else(|| {
            Error::invalid("this card does not say where the repository is: pass `repo`")
        })?;
    let suffix = format!("{:016x}", rand::random::<u64>());
    let bundle = format!("/tmp/cua-phone-diff-{suffix}.bundle");
    let tmp_ref = format!("refs/cua/phone-diff-{suffix}");
    let out = space
        .bash(
            &ht::bundle_commit_script(&path, &bundle, &card.head, &card.base, &tmp_ref),
            Duration::from_secs(120),
        )
        .await?;
    if out.stdout.contains("GONE") {
        return Err(Error::invalid(
            "the repository no longer holds the commit this approval is about",
        ));
    }
    if !out.stdout.lines().any(|l| l == "OK") {
        return Err(Error::invalid(format!(
            "could not read the repository: {}",
            out.stderr.lines().next().unwrap_or("git failed").trim()
        )));
    }
    let mut stat = String::new();
    let mut commits = 0u64;
    for l in out.stdout.lines() {
        if let Some(s) = l.strip_prefix("STAT ") {
            stat = s.to_string();
        } else if let Some(c) = l.strip_prefix("COMMITS ") {
            commits = c.trim().parse().unwrap_or(0);
        }
    }
    let (files, additions, deletions) = parse_shortstat(&stat);
    let binding = Binding {
        head: card.head.clone(),
        base: card.base.clone(),
        files,
        additions,
        deletions,
        commits,
    };
    let limits = PushLimits {
        max_bytes: BUNDLE_MAX_BYTES,
        download_timeout: Duration::from_secs(120),
    };
    let (tmp, file) = ht::fetch_bundle(space, &bundle, &limits)
        .await
        .map_err(Error::from)?;
    let v = ht::verify_bundle_ref(&file, &binding, &limits, &tmp_ref)
        .await
        .map_err(Error::from)?;
    drop(tmp);
    let v = Arc::new(v);
    let mut c = verified_cache().lock().expect("verified cache");
    if c.len() >= VERIFIED_KEPT
        && let Some(oldest) = c
            .iter()
            .min_by_key(|(_, (at, _))| *at)
            .map(|(k, _)| k.clone())
    {
        c.remove(&oldest);
    }
    c.insert(key, (Instant::now(), v.clone()));
    Ok(v)
}

// ------------------------------------------------------------------- tools

/// `space_changes`.
pub(crate) async fn space_changes(
    spaces: &Spaces,
    p: &Persistent,
    caller: &Caller,
    a: Value,
) -> Result<ToolOutcome> {
    let r = request(&a, "space_changes")?;
    let (space, id) = space_for(spaces, p, caller, &r.space).await?;
    if let Some(seq) = r.approval {
        let t = p.thread(caller, &r.space).await?;
        let card = card_of(t.store(), seq)?;
        let v = verified(&space, &id, seq, &card, r.repo.as_deref()).await?;
        let numstat = v
            .git(&[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "-M",
                "--numstat",
                card.base.as_str(),
                card.head.as_str(),
            ])
            .await
            .map_err(Error::invalid)?;
        let names = v
            .git(&[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "-M",
                "--name-status",
                card.base.as_str(),
                card.head.as_str(),
            ])
            .await
            .map_err(Error::invalid)?;
        let all = parse_diffstat(&numstat, &names);
        return Ok(ToolOutcome::json(&changes_json(
            r.repo
                .as_deref()
                .or(card.repo_path.as_deref())
                .unwrap_or(""),
            None,
            Some(&card.base),
            &card.head,
            &all,
            &HashMap::new(),
            0,
            true,
            Some(seq),
        )));
    }
    let text = script_output(
        space
            .bash(
                &changes_script(r.repo.as_deref(), r.base.as_deref()),
                Duration::from_secs(60),
            )
            .await?,
    )?;
    Ok(ToolOutcome::json(&parse_changes(&text)))
}

/// The shell script that lists the repository's changes against its base.
pub(crate) fn changes_script(repo: Option<&str>, base: Option<&str>) -> String {
    format!(
        "{}{TOP}{HEAD}{}\
         echo \"TOP $top\"; echo \"BRANCH $(git symbolic-ref --short -q HEAD)\"; echo \"HEAD $head\"; echo \"BASE $base\"; \
         echo \"BASENAME $(git symbolic-ref --short -q refs/remotes/origin/HEAD | sed 's#^origin/##')\"; \
         git -c core.quotepath=false diff --no-ext-diff --no-textconv --numstat -M \"$base\" -- | head -n 1000 | awk '{{print \"NUM\\t\" $0}}'; \
         git -c core.quotepath=false diff --no-ext-diff --no-textconv --name-status -M \"$base\" -- | head -n 1000 | awk '{{print \"NAM\\t\" $0}}'; \
         git -c core.quotepath=false diff --no-ext-diff --no-textconv --name-only --diff-filter=d \"$base\" -- | head -n {FILES_MAX} | while IFS= read -r f; do \
           [ -f \"$f\" ] && printf 'SZ\\t%s\\t%s\\n' \"$f\" \"$(wc -c < \"$f\" | tr -d ' ')\"; done; \
         git ls-files --others --exclude-standard | head -n {FILES_MAX} | while IFS= read -r f; do \
           [ -f \"$f\" ] || continue; s=$(wc -c < \"$f\" | tr -d ' '); \
           if [ \"$s\" -gt 0 ] && ! grep -Iq . \"$f\" 2>/dev/null; then printf 'UNB\\t%s\\t%s\\n' \"$f\" \"$s\"; \
           else printf 'UNT\\t%s\\t%s\\t%s\\n' \"$f\" \"$(wc -l < \"$f\" | tr -d ' ')\" \"$s\"; fi; done",
        enter(repo),
        base_of(base),
    )
}

/// Reads what [`changes_script`] printed.
pub(crate) fn parse_changes(text: &str) -> Value {
    let (mut numstat, mut names) = (String::new(), String::new());
    let (mut top, mut branch, mut head, mut base, mut base_name) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let mut sizes: HashMap<String, u64> = HashMap::new();
    let mut untracked: Vec<FileStat> = vec![];
    for l in text.lines() {
        if let Some(x) = l.strip_prefix("NUM\t") {
            numstat.push_str(x);
            numstat.push('\n');
        } else if let Some(x) = l.strip_prefix("NAM\t") {
            names.push_str(x);
            names.push('\n');
        } else if let Some(x) = l.strip_prefix("TOP ") {
            top = x.trim().to_string();
        } else if let Some(x) = l.strip_prefix("BRANCH ") {
            branch = x.trim().to_string();
        } else if let Some(x) = l.strip_prefix("HEAD ") {
            head = x.trim().to_string();
        } else if let Some(x) = l.strip_prefix("BASENAME ") {
            base_name = x.trim().to_string();
        } else if let Some(x) = l.strip_prefix("BASE ") {
            base = x.trim().to_string();
        } else if let Some(x) = l.strip_prefix("SZ\t") {
            let mut it = x.split('\t');
            if let (Some(path), Some(n)) =
                (it.next(), it.next().and_then(|n| n.trim().parse().ok()))
            {
                sizes.insert(path.to_string(), n);
            }
        } else if let Some(x) = l.strip_prefix("UNT\t") {
            let f: Vec<&str> = x.split('\t').collect();
            if let [path, lines, size] = f[..] {
                sizes.insert(path.to_string(), size.trim().parse().unwrap_or(0));
                untracked.push(FileStat {
                    path: path.to_string(),
                    status: "added".into(),
                    additions: Some(lines.trim().parse().unwrap_or(0)),
                    deletions: Some(0),
                    ..Default::default()
                });
            }
        } else if let Some(x) = l.strip_prefix("UNB\t") {
            let f: Vec<&str> = x.split('\t').collect();
            if let [path, size] = f[..] {
                sizes.insert(path.to_string(), size.trim().parse().unwrap_or(0));
                untracked.push(FileStat {
                    path: path.to_string(),
                    status: "added".into(),
                    binary: true,
                    ..Default::default()
                });
            }
        }
    }
    let mut all = parse_diffstat(&numstat, &names);
    all.extend(untracked);
    let cut_by_script = numstat.lines().count() >= 1000;
    changes_json(
        &top,
        Some(&branch),
        Some(if base_name.is_empty() {
            &base
        } else {
            &base_name
        }),
        &head,
        &all,
        &sizes,
        u64::from(cut_by_script),
        false,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn changes_json(
    repo: &str,
    branch: Option<&str>,
    base: Option<&str>,
    head: &str,
    all: &[FileStat],
    sizes: &HashMap<String, u64>,
    cut: u64,
    verified: bool,
    approval: Option<u64>,
) -> Value {
    let sum = |f: fn(&FileStat) -> Option<u64>| all.iter().map(|x| f(x).unwrap_or(0)).sum::<u64>();
    let files: Vec<Value> = all
        .iter()
        .take(FILES_MAX)
        .map(|f| file_json(f, sizes.get(&f.path).copied()))
        .collect();
    let mut out = json!({
        "repo": repo,
        "branch": branch.unwrap_or(""),
        "base": base.unwrap_or(""),
        "head": head,
        "head_sha": head.chars().take(7).collect::<String>(),
        "files": files,
        "files_total": all.len(),
        "additions_total": sum(|f| f.additions),
        "deletions_total": sum(|f| f.deletions),
        "truncated": all.len() > FILES_MAX || cut > 0,
        // `true`: computed on this machine from content checked against the
        // push approval's card; `false`: what the Space says.
        "verified": verified,
    });
    if let Some(a) = approval {
        out["approval"] = json!(a);
    }
    out
}

/// `space_file_diff`.
pub(crate) async fn file_diff(
    spaces: &Spaces,
    p: &Persistent,
    caller: &Caller,
    a: Value,
) -> Result<ToolOutcome> {
    let r = request(&a, "space_file_diff")?;
    let path = get_str(&a, "path")
        .filter(|p| valid_file(p))
        .ok_or_else(|| Error::invalid("space_file_diff: `path` is required"))?
        .to_string();
    let old_path = get_str(&a, "old_path")
        .filter(|p| valid_file(p))
        .map(str::to_string);
    let context = a
        .get("context")
        .and_then(Value::as_u64)
        .map_or(3, |c| c.min(u64::from(CONTEXT_MAX)));
    let (space, id) = space_for(spaces, p, caller, &r.space).await?;
    if let Some(seq) = r.approval {
        let t = p.thread(caller, &r.space).await?;
        let card = card_of(t.store(), seq)?;
        let v = verified(&space, &id, seq, &card, r.repo.as_deref()).await?;
        let ctx = format!("-U{context}");
        let mut args = vec![
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "-M",
            ctx.as_str(),
            card.base.as_str(),
            card.head.as_str(),
            "--",
            path.as_str(),
        ];
        if let Some(o) = &old_path {
            args.push(o.as_str());
        }
        let bytes = v.git_bytes(&args).await.map_err(Error::invalid)?;
        return Ok(ToolOutcome::json(&diff_json(&path, &bytes, true)));
    }
    let script = diff_script(
        r.repo.as_deref(),
        r.base.as_deref(),
        &path,
        old_path.as_deref(),
        context,
    );
    let out = space.bash(&script, Duration::from_secs(60)).await?;
    if let Some(line) = out
        .stdout
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("ERR "))
    {
        return Err(Error::invalid(line.trim().to_string()));
    }
    Ok(ToolOutcome::json(&diff_json(
        &path,
        out.stdout.as_bytes(),
        false,
    )))
}

/// The shell script that prints one file's unified diff against the base,
/// cut at [`DIFF_MAX`] plus a byte (to know it was cut).
pub(crate) fn diff_script(
    repo: Option<&str>,
    base: Option<&str>,
    path: &str,
    old_path: Option<&str>,
    context: u64,
) -> String {
    let q = sh_quote(path);
    let oldq = old_path.map(sh_quote).unwrap_or_default();
    format!(
        "{}{TOP}{HEAD}{}f={q}; \
         if git ls-files --error-unmatch -- \"$f\" >/dev/null 2>&1 || [ ! -e \"$f\" ]; then \
           git -c core.quotepath=false diff --no-ext-diff --no-textconv -M -U{context} \"$base\" -- \"$f\" {oldq}; \
         else git -c core.quotepath=false diff --no-index --no-ext-diff --no-textconv -U{context} -- /dev/null \"$f\"; fi | head -c {}",
        enter(repo),
        base_of(base),
        DIFF_MAX + 1,
    )
}

/// A diff as the wire shows it: the patch (cut at [`DIFF_MAX`]), or none for
/// a binary file.
fn diff_json(path: &str, raw: &[u8], verified: bool) -> Value {
    let text = String::from_utf8_lossy(raw);
    let binary = text
        .lines()
        .any(|l| l.starts_with("Binary files ") || l.starts_with("GIT binary patch"));
    let truncated = raw.len() > DIFF_MAX;
    let patch = if binary {
        String::new()
    } else {
        let cut = String::from_utf8_lossy(&raw[..raw.len().min(DIFF_MAX)]).into_owned();
        if truncated {
            // End at the last whole line.
            match cut.rfind('\n') {
                Some(i) => cut[..=i].to_string(),
                None => cut,
            }
        } else {
            cut
        }
    };
    json!({
        "path": path,
        "patch": patch,
        "truncated": truncated && !binary,
        "binary": binary,
        "verified": verified,
    })
}

/// `space_file_preview`.
pub(crate) async fn file_preview(
    spaces: &Spaces,
    p: &Persistent,
    caller: &Caller,
    a: Value,
) -> Result<ToolOutcome> {
    let r = request(&a, "space_file_preview")?;
    let path = get_str(&a, "path")
        .filter(|p| valid_file(p))
        .ok_or_else(|| Error::invalid("space_file_preview: `path` is required"))?
        .to_string();
    let rev = get_str(&a, "rev").unwrap_or("working").to_string();
    if !matches!(rev.as_str(), "working" | "head" | "base") {
        return Err(Error::invalid(
            "space_file_preview: `rev` is head, base or working",
        ));
    }
    let max = a
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map_or(PREVIEW_DEFAULT, |m| m.clamp(1, PREVIEW_MAX));
    let (space, id) = space_for(spaces, p, caller, &r.space).await?;
    if let Some(seq) = r.approval {
        if rev == "working" {
            return Err(Error::invalid(
                "a push approval's preview is of `head` or `base`",
            ));
        }
        let t = p.thread(caller, &r.space).await?;
        let card = card_of(t.store(), seq)?;
        let v = verified(&space, &id, seq, &card, r.repo.as_deref()).await?;
        let commit = if rev == "head" {
            &card.head
        } else {
            &card.base
        };
        let spec = format!("{commit}:{path}");
        let size: u64 = v
            .git(&["cat-file", "-s", spec.as_str()])
            .await
            .map_err(|_| Error::NotFound(format!("{path} at {rev}")))?
            .trim()
            .parse()
            .unwrap_or(0);
        let mut bytes = v
            .git_bytes(&["cat-file", "blob", spec.as_str()])
            .await
            .map_err(Error::invalid)?;
        let truncated = bytes.len() as u64 > max;
        bytes.truncate(max as usize);
        return Ok(ToolOutcome::json(&preview_json(
            &path, size, &bytes, truncated, true,
        )));
    }
    let script = preview_script(r.repo.as_deref(), r.base.as_deref(), &path, &rev, max);
    let text = script_output(space.bash(&script, Duration::from_secs(60)).await?)?;
    let size: u64 = text
        .lines()
        .find_map(|l| l.strip_prefix("SIZE "))
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    let data = text.split_once("DATA\n").map_or("", |(_, d)| d.trim());
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| Error::invalid("the Space answered a preview that is not base64"))?;
    let truncated = size > bytes.len() as u64;
    Ok(ToolOutcome::json(&preview_json(
        &path, size, &bytes, truncated, false,
    )))
}

/// The shell script that prints a file's size and its first `max` bytes in
/// base64, from the work tree (`working`) or a commit (`head`, `base`).
pub(crate) fn preview_script(
    repo: Option<&str>,
    base: Option<&str>,
    path: &str,
    rev: &str,
    max: u64,
) -> String {
    let q = sh_quote(path);
    let read = match rev {
        "working" => format!(
            "[ -f {q} ] || {{ echo 'ERR no such file'; exit 6; }}; echo \"SIZE $(wc -c < {q} | tr -d ' ')\"; echo DATA; head -c {max} {q} | base64 | tr -d '\\n'"
        ),
        "head" => format!(
            "spec=\"HEAD:\"{q}; git cat-file -e \"$spec\" 2>/dev/null || {{ echo 'ERR no such file'; exit 6; }}; echo \"SIZE $(git cat-file -s \"$spec\")\"; echo DATA; git cat-file blob \"$spec\" | head -c {max} | base64 | tr -d '\\n'"
        ),
        _ => format!(
            "spec=\"$base:\"{q}; git cat-file -e \"$spec\" 2>/dev/null || {{ echo 'ERR no such file'; exit 6; }}; echo \"SIZE $(git cat-file -s \"$spec\")\"; echo DATA; git cat-file blob \"$spec\" | head -c {max} | base64 | tr -d '\\n'"
        ),
    };
    format!("{}{TOP}{HEAD}{}{read}", enter(repo), base_of(base))
}

fn sniff_image(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

/// A preview as the wire shows it: text as text, an image as base64, and
/// nothing for anything else.
fn preview_json(path: &str, size: u64, bytes: &[u8], truncated: bool, verified: bool) -> Value {
    if let Some(mime) = sniff_image(bytes) {
        // A cut image does not draw: say so rather than send half of it.
        let content = if truncated {
            String::new()
        } else {
            base64::engine::general_purpose::STANDARD.encode(bytes)
        };
        return json!({"path": path, "mime": mime, "size": size, "encoding": "base64",
                      "content": content, "truncated": truncated, "binary": false, "verified": verified});
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(t) => Some(t),
        // A cut in the middle of a character is still text.
        Err(e) if truncated && e.valid_up_to() + 3 >= bytes.len() => {
            std::str::from_utf8(&bytes[..e.valid_up_to()]).ok()
        }
        Err(_) => None,
    }
    .filter(|t| !t.contains('\0'));
    match text {
        Some(t) => {
            json!({"path": path, "mime": mime_of(path).filter(|m| !m.starts_with("image/")).unwrap_or("text/plain"),
                          "size": size, "encoding": "utf8", "content": t, "truncated": truncated,
                          "binary": false, "verified": verified})
        }
        None => {
            json!({"path": path, "mime": "application/octet-stream", "size": size, "encoding": "base64",
                       "content": "", "truncated": truncated, "binary": true, "verified": verified})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked_before_they_reach_a_shell() {
        assert!(valid_rev("main") && valid_rev("origin/main~2") && valid_rev("v1.0^{commit}"));
        for bad in ["", "-x", "a b", "a;b", "$(x)", "a`b`", &"a".repeat(201)] {
            assert!(!valid_rev(bad), "{bad:?}");
        }
        assert!(valid_file("src/a b.rs"));
        for bad in ["", "-rf", "a\nb", "a\0b"] {
            assert!(!valid_file(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_diff_is_cut_at_a_line_and_a_binary_one_has_no_patch() {
        let small = diff_json("a.rs", b"diff --git a/a.rs b/a.rs\n+x\n", false);
        assert_eq!(small["truncated"], false);
        assert_eq!(small["binary"], false);
        assert!(small["patch"].as_str().unwrap().ends_with("+x\n"));
        let big = "+line\n".repeat(DIFF_MAX);
        let cut = diff_json("a.rs", big.as_bytes(), true);
        assert_eq!(cut["truncated"], true);
        assert_eq!(cut["verified"], true);
        let patch = cut["patch"].as_str().unwrap();
        assert!(
            patch.len() <= DIFF_MAX && patch.ends_with('\n'),
            "{}",
            patch.len()
        );
        let bin = diff_json("a.png", b"Binary files a/a.png and b/a.png differ\n", false);
        assert_eq!(
            (bin["binary"].clone(), bin["patch"].clone()),
            (json!(true), json!(""))
        );
    }

    #[test]
    fn previews_are_text_or_images_only() {
        let t = preview_json("a.md", 5, b"hello", false, false);
        assert_eq!(
            (
                t["encoding"].as_str(),
                t["content"].as_str(),
                t["binary"].as_bool()
            ),
            (Some("utf8"), Some("hello"), Some(false))
        );
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        png.extend_from_slice(&[0; 32]);
        let i = preview_json("a.png", 40, &png, false, false);
        assert_eq!(i["mime"], "image/png");
        assert_eq!(i["encoding"], "base64");
        assert!(!i["content"].as_str().unwrap().is_empty());
        // A cut image sends nothing it cannot draw.
        let cut = preview_json("a.png", 4_000_000, &png, true, false);
        assert_eq!(cut["content"], "");
        assert_eq!(cut["truncated"], true);
        let bin = preview_json("a.bin", 4, &[0, 1, 2, 3], false, false);
        assert_eq!(bin["binary"], true);
        assert_eq!(bin["content"], "");
        // Text cut in the middle of a character is still text.
        let s = "añb".as_bytes();
        let part = preview_json("a.txt", 9, &s[..2], true, false);
        assert_eq!(part["encoding"], "utf8");
    }

    #[test]
    fn the_file_list_carries_counts_and_sizes_and_caps() {
        let files: Vec<FileStat> = (0..(FILES_MAX + 5))
            .map(|i| FileStat {
                path: format!("f{i}.rs"),
                status: "modified".into(),
                additions: Some(2),
                deletions: Some(1),
                ..Default::default()
            })
            .collect();
        let mut sizes = HashMap::new();
        sizes.insert("f0.rs".to_string(), 99);
        let v = changes_json(
            "/home/u/app",
            Some("fix"),
            Some("main"),
            &"a".repeat(40),
            &files,
            &sizes,
            0,
            false,
            None,
        );
        assert_eq!(v["files"].as_array().unwrap().len(), FILES_MAX);
        assert_eq!(v["files_total"], FILES_MAX + 5);
        assert_eq!(v["additions_total"], 2 * (FILES_MAX + 5) as u64);
        assert_eq!(v["truncated"], true);
        assert_eq!(v["files"][0]["size"], 99);
        assert_eq!(v["head_sha"], "aaaaaaa");
        assert_eq!(v["verified"], false);
    }

    // ---- against a real repository: the scripts are the ones the Space runs

    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let o = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("git");
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn sh(script: &str, home: &Path) -> String {
        let o = Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("HOME", home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("sh");
        String::from_utf8_lossy(&o.stdout).to_string()
    }

    /// `~/app` with two commits and some work on top: a modified file, an
    /// untracked text file, an untracked binary file and an image.
    fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("app");
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "first"]);
        std::fs::write(dir.join("b.rs"), "fn main() {}\n").unwrap();
        std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "second"]);
        (home, dir)
    }

    #[test]
    fn the_changes_of_a_working_tree_are_listed_with_counts() {
        let (home, dir) = repo();
        std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        std::fs::write(dir.join("c.md"), "hello\nworld\n").unwrap();
        std::fs::write(dir.join("d.bin"), [0u8, 1, 2, 0, 3]).unwrap();
        let v = parse_changes(&sh(&changes_script(Some("app"), None), home.path()));
        assert_eq!(v["branch"], "main");
        assert_eq!(v["verified"], false);
        let files = v["files"].as_array().unwrap();
        let by = |p: &str| {
            files
                .iter()
                .find(|f| f["path"] == p)
                .unwrap_or_else(|| panic!("{p} in {files:?}"))
        };
        assert_eq!(
            (
                by("a.txt")["additions"].clone(),
                by("a.txt")["deletions"].clone()
            ),
            (json!(1), json!(0))
        );
        assert_eq!(by("a.txt")["status"], "modified");
        assert_eq!(by("a.txt")["size"], 19);
        assert_eq!(
            (
                by("c.md")["status"].clone(),
                by("c.md")["additions"].clone()
            ),
            (json!("added"), json!(2))
        );
        assert_eq!(by("d.bin")["binary"], true);
        assert_eq!(v["head"].as_str().unwrap().len(), 40);
        assert_eq!(v["head_sha"].as_str().unwrap().len(), 7);
    }

    #[test]
    fn a_commit_range_and_a_missing_repository_say_what_they_are() {
        let (home, dir) = repo();
        let first = git(&dir, &["rev-parse", "HEAD~1"]);
        let v = parse_changes(&sh(&changes_script(Some("app"), Some(&first)), home.path()));
        let files: Vec<&str> = v["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(files, ["a.txt", "b.rs"]);
        assert_eq!(v["additions_total"], 3);
        assert_eq!(v["deletions_total"], 1);
        // No such folder, not a repository, an unknown base: one line each.
        let none = sh(&changes_script(Some("nowhere"), None), home.path());
        assert!(none.contains("ERR no such folder"), "{none}");
        std::fs::create_dir(home.path().join("plain")).unwrap();
        let plain = sh(&changes_script(Some("plain"), None), home.path());
        assert!(plain.contains("ERR not a git repository"), "{plain}");
        let bad = sh(&changes_script(Some("app"), Some("nonesuch")), home.path());
        assert!(bad.contains("ERR unknown base"), "{bad}");
        // The most recently used checkout is found without naming it.
        let auto = parse_changes(&sh(&changes_script(None, None), home.path()));
        assert!(auto["repo"].as_str().unwrap().ends_with("/app"), "{auto}");
    }

    #[test]
    fn one_files_diff_and_previews() {
        let (home, dir) = repo();
        std::fs::write(dir.join("c.md"), "hello\n").unwrap();
        std::fs::write(
            dir.join("e.png"),
            [&[0x89u8, b'P', b'N', b'G'][..], &[0u8; 20][..]].concat(),
        )
        .unwrap();
        let first = git(&dir, &["rev-parse", "HEAD~1"]);
        // A tracked file against the first commit.
        let d = sh(
            &diff_script(Some("app"), Some(&first), "a.txt", None, 3),
            home.path(),
        );
        assert!(
            d.contains("-two") && d.contains("+TWO") && d.contains("+three"),
            "{d}"
        );
        let v = diff_json("a.txt", d.as_bytes(), false);
        assert_eq!(
            (v["binary"].clone(), v["truncated"].clone()),
            (json!(false), json!(false))
        );
        // An untracked file is all additions.
        let u = sh(
            &diff_script(Some("app"), None, "c.md", None, 3),
            home.path(),
        );
        assert!(u.contains("+hello"), "{u}");
        // A file in a folder with odd characters goes through the quoting.
        std::fs::create_dir_all(dir.join("it's here")).unwrap();
        std::fs::write(dir.join("it's here/x y.txt"), "q\n").unwrap();
        let odd = sh(
            &diff_script(Some("app"), None, "it's here/x y.txt", None, 3),
            home.path(),
        );
        assert!(odd.contains("+q"), "{odd}");
        // Previews: the work tree, the head commit and the base.
        let work = sh(
            &preview_script(Some("app"), None, "c.md", "working", 1000),
            home.path(),
        );
        let w = script_text_preview("c.md", &work);
        assert_eq!(
            (w["content"].as_str(), w["size"].as_u64()),
            (Some("hello\n"), Some(6))
        );
        let head = sh(
            &preview_script(Some("app"), None, "a.txt", "head", 1000),
            home.path(),
        );
        assert_eq!(
            script_text_preview("a.txt", &head)["content"],
            "one\nTWO\nthree\n"
        );
        let base = sh(
            &preview_script(Some("app"), Some(&first), "a.txt", "base", 1000),
            home.path(),
        );
        assert_eq!(script_text_preview("a.txt", &base)["content"], "one\ntwo\n");
        let img = sh(
            &preview_script(Some("app"), None, "e.png", "working", 1000),
            home.path(),
        );
        assert_eq!(script_text_preview("e.png", &img)["mime"], "image/png");
        // Cut at the limit, and a missing file says so.
        let cut = sh(
            &preview_script(Some("app"), None, "a.txt", "head", 4),
            home.path(),
        );
        let c = script_text_preview("a.txt", &cut);
        assert_eq!(
            (c["content"].as_str(), c["truncated"].as_bool()),
            (Some("one\n"), Some(true))
        );
        let gone = sh(
            &preview_script(Some("app"), None, "nope.txt", "working", 10),
            home.path(),
        );
        assert!(gone.contains("ERR no such file"), "{gone}");
    }

    /// What `file_preview` does with a script's output, without a Space.
    fn script_text_preview(path: &str, out: &str) -> Value {
        let size: u64 = out
            .lines()
            .find_map(|l| l.strip_prefix("SIZE "))
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        let data = out.split_once("DATA\n").map_or("", |(_, d)| d.trim());
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        preview_json(path, size, &bytes, size > bytes.len() as u64, false)
    }

    /// The push approval's path: the commit is bundled the way the Space
    /// does it, checked by the push's own check, and read by git here.
    #[tokio::test]
    async fn a_cards_commit_is_checked_and_then_read_from_the_checked_copy() {
        let (home, dir) = repo();
        let head = git(&dir, &["rev-parse", "HEAD"]);
        let base = git(&dir, &["rev-parse", "HEAD~1"]);
        // The checkout moves on after the card: the viewer still gets the card's commit.
        std::fs::write(dir.join("later.txt"), "later\n").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "third"]);
        let bundle = home.path().join("v.bundle");
        let out = sh(
            &ht::bundle_commit_script(
                dir.to_str().unwrap(),
                bundle.to_str().unwrap(),
                &head,
                &base,
                "refs/cua/phone-diff-t",
            ),
            home.path(),
        );
        assert!(out.lines().any(|l| l == "OK"), "{out}");
        assert!(
            git(&dir, &["for-each-ref", "refs/cua"]).is_empty(),
            "the temporary ref is gone"
        );
        let mut stat = String::new();
        let mut commits = 0;
        for l in out.lines() {
            if let Some(x) = l.strip_prefix("STAT ") {
                stat = x.to_string();
            } else if let Some(x) = l.strip_prefix("COMMITS ") {
                commits = x.trim().parse().unwrap();
            }
        }
        let (files, additions, deletions) = parse_shortstat(&stat);
        let binding = Binding {
            head: head.clone(),
            base: base.clone(),
            files,
            additions,
            deletions,
            commits,
        };
        let limits = PushLimits::default();
        let v = ht::verify_bundle_ref(&bundle, &binding, &limits, "refs/cua/phone-diff-t")
            .await
            .map_err(|e| e.to_string())
            .expect("the bundle checks out");
        let numstat = v
            .git(&[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "-M",
                "--numstat",
                &base,
                &head,
            ])
            .await
            .unwrap();
        let names = v
            .git(&[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "-M",
                "--name-status",
                &base,
                &head,
            ])
            .await
            .unwrap();
        let all = parse_diffstat(&numstat, &names);
        assert_eq!(
            all.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["a.txt", "b.rs"]
        );
        assert!(
            v.git(&["cat-file", "blob", &format!("{head}:later.txt")])
                .await
                .is_err(),
            "the third commit is not in it"
        );
        let patch = v
            .git_bytes(&[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "-M",
                "-U3",
                &base,
                &head,
                "--",
                "a.txt",
            ])
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&patch).contains("+TWO"));
        // Numbers that are not the commits' make the check refuse.
        let wrong = Binding {
            additions: additions + 5,
            ..binding.clone()
        };
        let refused =
            ht::verify_bundle_ref(&bundle, &wrong, &limits, "refs/cua/phone-diff-t").await;
        assert!(refused.is_err());
        // So does the wrong ref name.
        assert!(
            ht::verify_bundle_ref(&bundle, &binding, &limits, "HEAD")
                .await
                .is_err()
        );
    }
}
