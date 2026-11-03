// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What the host measures for a push card: the diffstat per file (renames,
//! binary files, a very large diff), the commits the branch carries and the
//! branch it goes into. The host sends the data with the approval; the app
//! core (`cua-spaces-app-core::thread_cards`) turns it into the words and
//! bars every app shows.
//!
//! Nothing here reads the GitHub sign-in: the account is a plain string the
//! caller passes.

use serde_json::{Value, json};
use std::time::Duration;

use crate::persistent::host_tools::sh_quote;
use cua_spaces::Result;

/// Files kept for the card (the card lists fewer; the totals count all).
pub const KEEP_FILES: usize = 60;
/// Commits kept for the card.
pub const KEEP_COMMITS: usize = 12;
/// Diff lines the host reads before it trusts `--shortstat` for the totals.
pub const NUMSTAT_LINES: usize = 5000;

/// One changed file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileStat {
    pub path: String,
    pub old_path: Option<String>,
    /// `added`, `modified`, `deleted` or `renamed`.
    pub status: String,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub binary: bool,
}

/// One commit.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommitStat {
    pub sha: String,
    pub author: String,
    pub subject: String,
}

/// Everything measured in the repository.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Detail {
    pub files: Vec<FileStat>,
    pub files_total: u64,
    pub additions_total: u64,
    pub deletions_total: u64,
    pub commits: Vec<CommitStat>,
    pub commits_total: u64,
    /// The remote's default branch, when the clone knows it.
    pub base: Option<String>,
}

/// The two sides of a `--numstat` path: `a/{b => c}/d`, `a => b` and a plain
/// path. Returns (new path, old path).
pub fn numstat_path(field: &str) -> (String, Option<String>) {
    if let (Some(open), Some(close)) = (field.find('{'), field.find('}'))
        && open < close
        && let Some((from, to)) = field[open + 1..close].split_once(" => ")
    {
        let (pre, post) = (&field[..open], &field[close + 1..]);
        let join = |mid: &str| format!("{pre}{mid}{post}").replace("//", "/");
        return (join(to), Some(join(from)));
    }
    match field.split_once(" => ") {
        Some((from, to)) => (to.to_string(), Some(from.to_string())),
        None => (field.to_string(), None),
    }
}

/// Parses `git diff --numstat -M` and `git diff --name-status -M` (both
/// without `-z`, quoting off) into files. A binary file has `-` counts.
/// Lines that do not parse are skipped, never guessed at.
pub fn parse_diffstat(numstat: &str, name_status: &str) -> Vec<FileStat> {
    use std::collections::HashMap;
    // status letter by new path (and the old path of a rename)
    let mut status: HashMap<String, (char, Option<String>)> = HashMap::new();
    for l in name_status.lines() {
        let mut p = l.split('\t');
        let (Some(code), Some(first)) = (p.next(), p.next()) else {
            continue;
        };
        let letter = code.chars().next().unwrap_or('M');
        match (letter, p.next()) {
            ('R' | 'C', Some(new)) => {
                status.insert(new.to_string(), ('R', Some(first.to_string())));
            }
            _ => {
                status.insert(first.to_string(), (letter, None));
            }
        }
    }
    let mut out = Vec::new();
    for l in numstat.lines() {
        let mut p = l.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (p.next(), p.next(), p.next()) else {
            continue;
        };
        let (new, old) = numstat_path(path);
        let binary = a == "-" && d == "-";
        let (additions, deletions) = if binary {
            (None, None)
        } else {
            match (a.parse::<u64>(), d.parse::<u64>()) {
                (Ok(a), Ok(d)) => (Some(a), Some(d)),
                _ => continue,
            }
        };
        let (letter, from) = status.get(&new).cloned().unwrap_or(('M', None));
        let old_path = from.or(old);
        out.push(FileStat {
            status: match letter {
                'A' => "added",
                'D' => "deleted",
                'R' => "renamed",
                _ if old_path.is_some() => "renamed",
                _ => "modified",
            }
            .into(),
            path: new,
            old_path,
            additions,
            deletions,
            binary,
        });
    }
    out
}

/// `3 files changed, 12 insertions(+), 3 deletions(-)` as (files, added, removed).
pub fn parse_shortstat(s: &str) -> (u64, u64, u64) {
    let n = |word: &str| {
        s.split(',')
            .find(|part| part.contains(word))
            .and_then(|part| part.split_whitespace().next())
            .and_then(|x| x.parse().ok())
            .unwrap_or(0)
    };
    (n("file"), n("insertion"), n("deletion"))
}

/// Reads the measurements out of the script's output (`MEASURE_SCRIPT`).
pub fn parse_detail(out: &str) -> Detail {
    let (mut numstat, mut names, mut shortstat) = (String::new(), String::new(), String::new());
    let mut d = Detail::default();
    for l in out.lines() {
        if let Some(x) = l.strip_prefix("NUM\t") {
            numstat.push_str(x);
            numstat.push('\n');
        } else if let Some(x) = l.strip_prefix("NAM\t") {
            names.push_str(x);
            names.push('\n');
        } else if let Some(x) = l.strip_prefix("SHORT ") {
            shortstat = x.to_string();
        } else if let Some(x) = l.strip_prefix("COMMITS ") {
            d.commits_total = x.trim().parse().unwrap_or(0);
        } else if let Some(x) = l.strip_prefix("BASE ") {
            d.base = Some(x.trim().to_string()).filter(|b| !b.is_empty());
        } else if let Some(x) = l.strip_prefix("LOG\t") {
            let mut p = x.splitn(3, '\t');
            if let (Some(sha), Some(author), Some(subject)) = (p.next(), p.next(), p.next()) {
                d.commits.push(CommitStat {
                    sha: sha.into(),
                    author: author.into(),
                    subject: subject.into(),
                });
            }
        }
    }
    let all = parse_diffstat(&numstat, &names);
    let (sf, sa, sd) = parse_shortstat(&shortstat);
    // A very large diff: the script cut the list, so the totals come from
    // `--shortstat`, which counts every file.
    let cut = all.len() >= NUMSTAT_LINES;
    d.files_total = if cut {
        sf.max(all.len() as u64)
    } else {
        all.len() as u64
    };
    d.additions_total = if cut {
        sa
    } else {
        all.iter()
            .map(|f| f.additions.unwrap_or(0))
            .fold(0u64, u64::saturating_add)
    };
    d.deletions_total = if cut {
        sd
    } else {
        all.iter()
            .map(|f| f.deletions.unwrap_or(0))
            .fold(0u64, u64::saturating_add)
    };
    d.files = all.into_iter().take(KEEP_FILES).collect();
    d.commits.truncate(KEEP_COMMITS);
    d.commits_total = d.commits_total.max(d.commits.len() as u64);
    d
}

/// The shell script that prints what [`parse_detail`] reads. `path` is the
/// repository in the Space (already checked clean by the caller); `head` and
/// `base` are the full commit hashes the host measured, so the files and the
/// commits listed are those of that commit and no other, whatever the branch
/// is by now.
fn script(path: &str, head: &str, base: &str) -> String {
    let p = sh_quote(path);
    let h = sh_quote(head);
    let b = sh_quote(base);
    format!(
        "cd {p}; head={h}; base={b}; \
         basebranch=$(git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null | sed 's#^origin/##'); \
         echo \"BASE $basebranch\"; \
         if git cat-file -e \"$base^{{commit}}\" 2>/dev/null; then range=\"$base..$head\"; else range=\"$head\"; fi; \
         echo \"SHORT $(git diff --shortstat $base $head)\"; \
         git -c core.quotepath=false diff --numstat -M $base $head | head -n {NUMSTAT_LINES} | awk '{{print \"NUM\\t\" $0}}'; \
         git -c core.quotepath=false diff --name-status -M $base $head | head -n {NUMSTAT_LINES} | awk '{{print \"NAM\\t\" $0}}'; \
         echo \"COMMITS $(git rev-list --count $range)\"; \
         git log --format='LOG%x09%H%x09%an%x09%s' -n {KEEP_COMMITS} $range"
    )
}

/// Measures the diffstat, commits and base branch of the commit `head`
/// (compared with `base`, both full hashes) of the repository at `path` in
/// `space`. Read-only. An error means the card goes without them.
pub async fn measure_detail(
    space: &cua_spaces::Space,
    path: &str,
    head: &str,
    base: &str,
) -> Result<Detail> {
    if !crate::persistent::host_tools::is_full_sha(head)
        || !crate::persistent::host_tools::is_full_sha(base)
    {
        return Err(cua_spaces::Error::invalid(
            "the card needs full commit hashes",
        ));
    }
    let out = space
        .bash(&script(path, head, base), Duration::from_secs(60))
        .await?;
    Ok(parse_detail(&out.stdout))
}

/// The `card` data of the approval entry (`ExportCardInput` in the app
/// core), from the request and what was measured.
#[allow(clippy::too_many_arguments)]
pub fn card_json(
    agent: &str,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    head: &str,
    base_sha: &str,
    pr: Option<(&str, &str)>,
    link_only: bool,
    account: Option<&str>,
    from: &str,
    d: &Detail,
) -> Value {
    json!({
        "agent": agent,
        "repo": repo,
        "branch": branch,
        "base": base.map(String::from).or_else(|| d.base.clone()),
        "head": head,
        // The full hashes the host bound the approval to: what is pushed is
        // checked against them after the answer.
        "baseSha": base_sha,
        "files": d.files.iter().map(|f| json!({
            "path": f.path, "oldPath": f.old_path, "status": f.status,
            "additions": f.additions, "deletions": f.deletions, "binary": f.binary,
        })).collect::<Vec<_>>(),
        "filesTotal": d.files_total,
        "additionsTotal": d.additions_total,
        "deletionsTotal": d.deletions_total,
        "commits": d.commits.iter().map(|c| json!({
            "sha": c.sha, "author": c.author, "subject": c.subject,
        })).collect::<Vec<_>>(),
        "commitsTotal": d.commits_total,
        "pr": pr.map(|(t, b)| json!({"title": t, "body": b})),
        "linkOnly": link_only,
        "account": account,
        "from": from,
    })
}

/// The approval's `card` for a push, measured in the Space; `None` when the
/// repository could not be read for it (the card is then text only).
#[allow(clippy::too_many_arguments)]
pub async fn for_push(
    space: &cua_spaces::Space,
    path: &str,
    agent: &str,
    repo: &str,
    branch: &str,
    base: Option<&str>,
    head: &str,
    base_sha: &str,
    pr: Option<(&str, &str)>,
    link_only: bool,
    account: Option<&str>,
    from: &str,
) -> Option<Value> {
    match measure_detail(space, path, head, base_sha).await {
        Ok(d) => Some(card_json(
            agent, repo, branch, base, head, base_sha, pr, link_only, account, from, &d,
        )),
        Err(e) => {
            tracing::warn!("push card: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_carries_the_full_hashes_the_approval_is_bound_to() {
        let head = "0123456789abcdef0123456789abcdef01234567";
        let base = "89abcdef0123456789abcdef0123456789abcdef";
        let v = card_json(
            "claude",
            "acme/app",
            "fix",
            None,
            head,
            base,
            None,
            false,
            None,
            "Space",
            &Detail::default(),
        );
        assert_eq!(
            (v["head"].as_str(), v["baseSha"].as_str()),
            (Some(head), Some(base))
        );
    }

    #[test]
    fn numstat_paths_in_every_git_form() {
        assert_eq!(numstat_path("src/a.py"), ("src/a.py".into(), None));
        assert_eq!(
            numstat_path("old.py => new.py"),
            ("new.py".into(), Some("old.py".into()))
        );
        assert_eq!(
            numstat_path("src/{old => new}/a.py"),
            ("src/new/a.py".into(), Some("src/old/a.py".into()))
        );
        assert_eq!(
            numstat_path("src/{ => sub}/a.py"),
            ("src/sub/a.py".into(), Some("src/a.py".into()))
        );
        assert_eq!(
            numstat_path("src/{sub => }/a.py"),
            ("src/a.py".into(), Some("src/sub/a.py".into()))
        );
    }

    #[test]
    fn parses_added_deleted_renamed_binary_and_empty_changes() {
        let num = "9\t3\tsrc/retry.py\n21\t0\ttests/test_retry.py\n0\t14\tdocs/old.md\n-\t-\timg/logo.png\n0\t0\tsrc/{http.py => transport.py}\n";
        let nam = "M\tsrc/retry.py\nA\ttests/test_retry.py\nD\tdocs/old.md\nM\timg/logo.png\nR100\tsrc/http.py\tsrc/transport.py\n";
        let f = parse_diffstat(num, nam);
        assert_eq!(f.len(), 5);
        assert_eq!(
            (f[0].status.as_str(), f[0].additions, f[0].deletions),
            ("modified", Some(9), Some(3))
        );
        assert_eq!(f[1].status, "added");
        assert_eq!(f[2].status, "deleted");
        assert!(f[3].binary && f[3].additions.is_none());
        assert_eq!(
            (f[4].status.as_str(), f[4].old_path.as_deref()),
            ("renamed", Some("src/http.py"))
        );
        assert!(parse_diffstat("", "").is_empty(), "no change, no files");
        assert!(
            parse_diffstat("garbage\nx\ty\n", "").is_empty(),
            "malformed lines are skipped"
        );
    }

    #[test]
    fn a_very_large_diff_takes_its_totals_from_shortstat() {
        let mut out = String::from(
            "SHORT 12345 files changed, 4000000 insertions(+), 99000 deletions(-)\nCOMMITS 31\nBASE main\n",
        );
        for n in 0..NUMSTAT_LINES {
            out.push_str(&format!(
                "NUM\t1000\t20\tpkg/f{n}.rs\nNAM\tM\tpkg/f{n}.rs\n"
            ));
        }
        out.push_str("LOG\t0a1b\tHermes\tBack off\n");
        let d = parse_detail(&out);
        assert_eq!(
            (d.files_total, d.additions_total, d.deletions_total),
            (12345, 4_000_000, 99_000)
        );
        assert_eq!(d.files.len(), KEEP_FILES);
        assert_eq!(
            (d.commits.len(), d.commits_total, d.base.as_deref()),
            (1, 31, Some("main"))
        );
    }

    #[test]
    fn small_diffs_sum_their_files_and_zero_changes_are_zero() {
        let d = parse_detail(
            "SHORT 2 files changed, 12 insertions(+), 3 deletions(-)\nNUM\t9\t3\ta\nNAM\tM\ta\nNUM\t3\t0\tb\nNAM\tA\tb\nCOMMITS 1\nLOG\tabc\tHermes\tx\n",
        );
        assert_eq!(
            (d.files_total, d.additions_total, d.deletions_total),
            (2, 12, 3)
        );
        let z = parse_detail("SHORT \nCOMMITS 0\nBASE \n");
        assert_eq!(
            (z.files_total, z.additions_total, z.deletions_total, z.base),
            (0, 0, 0, None)
        );
    }

    /// A real repository with a real diff: the script's output parses to what git did.
    #[test]
    fn a_real_repository_measures_to_its_own_diff() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_AUTHOR_NAME", "Hermes")
                .env("GIT_AUTHOR_EMAIL", "h@h")
                .env("GIT_COMMITTER_NAME", "Hermes")
                .env("GIT_COMMITTER_EMAIL", "h@h")
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            String::from_utf8_lossy(&o.stdout).to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.path().join("cart.py"), "a\nb\nc\n").unwrap();
        std::fs::write(dir.path().join("old.txt"), "x\n".repeat(20)).unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        std::fs::write(dir.path().join("cart.py"), "a\nB\nc\nd\n").unwrap();
        std::fs::create_dir(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_cart.py"), "t\n").unwrap();
        std::fs::rename(dir.path().join("old.txt"), dir.path().join("new.txt")).unwrap();
        std::fs::write(dir.path().join("logo.png"), [0u8, 159, 146, 150]).unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "Fix the cart total"]);
        let sh = std::process::Command::new("sh")
            .arg("-c")
            .arg(script(
                dir.path().to_str().unwrap(),
                git(&["rev-parse", "HEAD"]).trim(),
                git(&["rev-parse", "HEAD~1"]).trim(),
            ))
            .output()
            .unwrap();
        let d = parse_detail(&String::from_utf8_lossy(&sh.stdout));
        let by = |p: &str| {
            d.files
                .iter()
                .find(|f| f.path == p)
                .unwrap_or_else(|| panic!("{p} in {:?}", d.files))
        };
        assert_eq!(d.files_total, 4);
        assert_eq!(
            (by("cart.py").additions, by("cart.py").deletions),
            (Some(2), Some(1))
        );
        assert_eq!(by("tests/test_cart.py").status, "added");
        assert_eq!(
            (
                by("new.txt").status.as_str(),
                by("new.txt").old_path.as_deref()
            ),
            ("renamed", Some("old.txt"))
        );
        assert!(by("logo.png").binary);
        assert_eq!(
            (
                d.commits.len(),
                d.commits[0].subject.as_str(),
                d.commits[0].author.as_str()
            ),
            (1, "Fix the cart total", "Hermes")
        );
    }

    /// Writes the card data a real push would carry, from a real clone of a
    /// local bare remote with a real two-commit diff, to `$CUA_CARD_OUT`
    /// (the screenshots for the pull request use it). Run by hand:
    /// `CUA_CARD_OUT=/path/card.json cargo test -p cua-spaces-ext --lib write_the_real_card -- --ignored`
    #[test]
    #[ignore = "writes a file for the screenshots; run by hand"]
    #[cfg(unix)]
    fn write_the_real_card() {
        let Ok(out) = std::env::var("CUA_CARD_OUT") else {
            return;
        };
        let root = tempfile::tempdir().unwrap();
        let run = |dir: &std::path::Path, args: &[&str]| {
            let o = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "Claude Code")
                .env("GIT_AUTHOR_EMAIL", "c@c")
                .env("GIT_COMMITTER_NAME", "Claude Code")
                .env("GIT_COMMITTER_EMAIL", "c@c")
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        };
        let remote = root.path().join("remotes/acme/app.git");
        std::fs::create_dir_all(&remote).unwrap();
        run(&remote, &["init", "-q", "--bare", "-b", "main"]);
        let work = root.path().join("work");
        run(
            root.path(),
            &["clone", "-q", remote.to_str().unwrap(), "work"],
        );
        let w = |p: &str, c: &str| {
            let f = work.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, c).unwrap();
        };
        let numbered = |n: usize, tag: &str| {
            (1..=n)
                .map(|i| format!("{tag} line {i}\n"))
                .collect::<String>()
        };
        w("src/net/ws_reconnect.py", &numbered(24, "reconnect"));
        w("docs/retry-notes.md", &numbered(14, "note"));
        w("src/net/http.py", &numbered(30, "http"));
        w("assets/status.png", "\u{0}PNG-old");
        run(&work, &["checkout", "-q", "-b", "main"]);
        run(&work, &["add", "."]);
        run(&work, &["commit", "-q", "-m", "Start"]);
        run(&work, &["push", "-q", "origin", "main"]);
        run(&work, &["remote", "set-head", "origin", "main"]);
        run(&work, &["checkout", "-q", "-b", "fix/retry-backoff"]);
        let mut edited = numbered(24, "reconnect");
        edited = edited
            .replace("reconnect line 5\n", "reconnect line 5, now with backoff\n")
            .replace("reconnect line 9\n", "");
        edited.push_str(&numbered(8, "backoff"));
        w("src/net/ws_reconnect.py", &edited);
        w("tests/test_ws_reconnect.py", &numbered(21, "check"));
        run(&work, &["add", "."]);
        run(
            &work,
            &["commit", "-q", "-m", "Back off between reconnect retries"],
        );
        std::fs::remove_file(work.join("docs/retry-notes.md")).unwrap();
        run(&work, &["mv", "src/net/http.py", "src/net/transport.py"]);
        w("assets/status.png", "\u{0}PNG-new-bytes\u{1}\u{2}");
        run(&work, &["add", "-A"]);
        run(
            &work,
            &[
                "commit",
                "-q",
                "-m",
                "Test the reconnect delays and tidy the net folder",
            ],
        );
        // The account line is the credential chain's own: a fake `gh` with one
        // account on PATH (as the sign-in chain's test does), asked who is signed in.
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let gh = bin.join("gh");
        std::fs::write(
            &gh,
            "#!/bin/sh\ncase \"$1 $2\" in\n  \"auth status\") echo '{\"hosts\":{\"github.com\":[{\"state\":\"success\",\"active\":true,\"host\":\"github.com\",\"login\":\"ada\",\"scopes\":\"repo\"}]}}'; exit 0;;\nesac\n[ \"$1\" = \"--version\" ] && echo \"gh version 2.101.0\" && exit 0\nexit 2\n",
        )
        .unwrap();
        std::fs::set_permissions(&gh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        // SAFETY: an ignored, by-hand test run alone.
        unsafe {
            std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
            std::env::remove_var("GH_TOKEN");
            std::env::remove_var("GITHUB_TOKEN");
        }
        let conn = crate::persistent::host_tools::GithubConnection::from_host("github.com");
        let account = conn.account(false);
        assert_eq!(account, "ada, signed in with gh");
        let sh = std::process::Command::new("sh")
            .arg("-c")
            .arg(script(
                work.to_str().unwrap(),
                String::from_utf8_lossy(
                    &std::process::Command::new("git")
                        .args(["rev-parse", "HEAD"])
                        .current_dir(&work)
                        .output()
                        .unwrap()
                        .stdout,
                )
                .trim(),
                String::from_utf8_lossy(
                    &std::process::Command::new("git")
                        .args(["merge-base", "HEAD", "origin/HEAD"])
                        .current_dir(&work)
                        .output()
                        .unwrap()
                        .stdout,
                )
                .trim(),
            ))
            .output()
            .unwrap();
        let d = parse_detail(&String::from_utf8_lossy(&sh.stdout));
        assert_eq!(
            d.base.as_deref(),
            Some("main"),
            "the clone knows its remote's default branch"
        );
        let card = card_json(
            "Claude Code",
            "acme/app",
            "fix/retry-backoff",
            None,
            "0a1b2c3d4e5f60718293",
            "0a1b2c3d4e5f60718293",
            Some((
                "Back off between reconnect retries",
                "Reconnects now wait 1s, 2s, 4s.\n\nFixes the flaky sync test.\nAdds tests/test_ws_reconnect.py.",
            )),
            false,
            Some(account.as_str()),
            "ubuntu-build",
            &d,
        );
        std::fs::write(out, serde_json::to_string_pretty(&card).unwrap()).unwrap();
    }
}
