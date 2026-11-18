// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The cards in a Space's thread: what the host asks before something
//! leaves the Space (a push to GitHub, with or without a pull request), and
//! who the thread shows as the author of what Cua itself writes.
//!
//! The host measures the change (the diffstat per file, the commits, the
//! branches, the account it will use) and sends it as data in the approval
//! entry ([`ExportCardInput`]). Every word and number the card shows is
//! made here ([`export_card_view`]), so the desktop, web and Electron cards
//! read alike: the repository and the action in plain words, `branch into
//! base`, `+12 -3` totals with a five-square bar per file, the commits, the
//! pull request that will open, the account, and the one line the card
//! collapses to once it is answered. The shells only lay the pieces out and
//! colour them with the theme's diff tokens.

use serde::{Deserialize, Serialize};

/// Longest commit subject shown, in characters.
pub const SUBJECT_MAX: usize = 72;
/// Longest pull request title shown, in characters.
pub const TITLE_MAX: usize = 120;
/// Files and commits the opened card lists (the rest is "+N more").
pub const SHOW_FILES: usize = 3;
pub const SHOW_COMMITS: usize = 3;
/// Lines of a pull request description the preview shows.
pub const BODY_LINES: usize = 2;
/// Longest preview line, in characters.
pub const BODY_LINE_MAX: usize = 110;

/// One changed file, as the host measured it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CardFile {
    pub path: String,
    /// The path before a rename.
    pub old_path: Option<String>,
    /// `added`, `modified`, `deleted` or `renamed`.
    pub status: String,
    /// Lines added; `None` for a binary file.
    pub additions: Option<u64>,
    /// Lines removed; `None` for a binary file.
    pub deletions: Option<u64>,
    pub binary: bool,
}

/// One commit that the push carries.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CardCommit {
    pub sha: String,
    pub author: String,
    pub subject: String,
}

/// The pull request that will be opened.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CardPr {
    pub title: String,
    pub body: String,
    /// Opened as a draft.
    pub draft: bool,
}

/// What the host measured for a push, sent with the approval.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportCardInput {
    /// Who asks (`Hermes`).
    pub agent: String,
    /// `owner/name` on GitHub.
    pub repo: String,
    pub branch: String,
    /// The branch the change goes into, when known.
    pub base: Option<String>,
    /// The commit the branch ends at.
    pub head: String,
    pub files: Vec<CardFile>,
    /// Files changed in all (more than `files` when it was cut).
    pub files_total: u64,
    pub additions_total: u64,
    pub deletions_total: u64,
    pub commits: Vec<CardCommit>,
    pub commits_total: u64,
    /// A pull request will be opened, with this title and description.
    pub pr: Option<CardPr>,
    /// The connection can push but not open the pull request: the person gets a link.
    pub link_only: bool,
    /// The account the push uses, in plain words (`ada, signed in with gh`).
    pub account: Option<String>,
    /// The Space the repository is in.
    pub from: String,
}

/// One square of a file's bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Square {
    Add,
    Del,
    None,
}

/// A file row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRow {
    pub name: String,
    /// The folder, `src/app/`, or empty.
    pub dir: String,
    pub status: String,
    /// `+10`, or empty when nothing was added (and for a binary file).
    pub additions: String,
    /// `-3`, or empty.
    pub deletions: String,
    /// `renamed from old/name.py`, `binary file`, or empty.
    pub note: String,
    /// Five squares, GitHub's proportion bar.
    pub bar: [Square; 5],
}

/// A commit row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRow {
    /// The first seven characters.
    pub sha: String,
    pub author: String,
    pub subject: String,
}

/// The pull request preview, drawn like a pull request's header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrView {
    pub title: String,
    /// `Open` or `Draft`: the pill.
    pub state: String,
    /// At most two lines of the description.
    pub body: Vec<String>,
}

/// The diff line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    /// `5 files`.
    pub files: String,
    /// `+12`, or empty.
    pub additions: String,
    /// `-3`, or empty.
    pub deletions: String,
}

/// The card, ready to lay out. Collapsed it is: the headline, `branch into
/// base`, the diff line, the account and the buttons; the files, commits and
/// pull request preview are behind one `Show changes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportCardView {
    pub repo: String,
    /// `Hermes wants to push a branch and open a pull request on acme/app.`
    pub headline: String,
    pub branch: String,
    pub base: Option<String>,
    /// `fix/retry-backoff into main`.
    pub branch_line: String,
    pub totals: Totals,
    /// `Cua pushes it from this computer as ada, signed in with gh.`
    pub account: Option<String>,
    /// There is something to show behind the expander.
    pub expandable: bool,
    /// `Show changes`.
    pub show_label: String,
    /// `Hide changes`.
    pub hide_label: String,
    /// The first three files.
    pub files: Vec<FileRow>,
    /// `+12 more`.
    pub more_files: Option<String>,
    /// `2 commits`.
    pub commits_heading: String,
    /// The first three commits.
    pub commits: Vec<CommitRow>,
    pub more_commits: Option<String>,
    /// Only when a pull request will be opened and it has a title.
    pub pr: Option<PrView>,
    /// `Push and open PR`, or `Push` when only a branch is pushed.
    pub allow: String,
    /// `Not now`.
    pub deny: String,
    /// The one muted line the card becomes after a yes.
    pub result_approved: String,
    /// The one muted line after a no.
    pub result_declined: String,
}

/// The text lines of the collapsed card, in order: what a person reads
/// before opening it (the budget the tests hold).
pub fn collapsed_lines(v: &ExportCardView) -> Vec<String> {
    let mut out = vec![v.headline.clone(), v.branch_line.clone()];
    let diff = [
        v.totals.files.as_str(),
        v.totals.additions.as_str(),
        v.totals.deletions.as_str(),
    ]
    .iter()
    .filter(|x| !x.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join(" ");
    out.push(diff);
    out.extend(v.account.clone());
    if v.expandable {
        out.push(v.show_label.clone());
    }
    out.push(format!("{} {}", v.allow, v.deny));
    out
}

/// `12345` as `12,345`.
pub fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

/// `text` on one line, cut to `max` characters with an ellipsis.
fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let mut c: String = flat.chars().take(max.saturating_sub(1)).collect();
    c.truncate(c.trim_end().len());
    c.push('\u{2026}');
    c
}

/// GitHub's five-square bar for `add` and `del` lines: squares in
/// proportion, a changed side never rounds to nothing, the rest neutral.
/// No change, and a binary file, is five neutral squares.
pub fn bar(add: u64, del: u64) -> [Square; 5] {
    let total = add.saturating_add(del);
    if total == 0 {
        return [Square::None; 5];
    }
    let share = |n: u64| ((n as u128 * 5 + total as u128 / 2) / total as u128) as u64;
    let (mut a, mut d) = (share(add), share(del));
    if add > 0 && a == 0 {
        a = 1;
    }
    if del > 0 && d == 0 {
        d = 1;
    }
    while a + d > 5 {
        if a >= d {
            a -= 1;
        } else {
            d -= 1;
        }
    }
    let mut out = [Square::None; 5];
    for (i, s) in out.iter_mut().enumerate() {
        let i = i as u64;
        *s = if i < a {
            Square::Add
        } else if i < a + d {
            Square::Del
        } else {
            Square::None
        };
    }
    out
}

/// `src/app/cart.py` as (`cart.py`, `src/app/`); a long folder keeps its last two parts.
fn split_path(p: &str) -> (String, String) {
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    match parts.split_last() {
        None => (p.to_string(), String::new()),
        Some((name, [])) => (name.to_string(), String::new()),
        Some((name, dirs)) => {
            let keep = &dirs[dirs.len().saturating_sub(2)..];
            let lead = if dirs.len() > 2 { "\u{2026}/" } else { "" };
            (name.to_string(), format!("{lead}{}/", keep.join("/")))
        }
    }
}

fn file_row(f: &CardFile) -> FileRow {
    let (name, dir) = split_path(&f.path);
    let binary = f.binary || (f.additions.is_none() && f.deletions.is_none());
    let (a, d) = (f.additions.unwrap_or(0), f.deletions.unwrap_or(0));
    let note = match (&f.old_path, binary) {
        (Some(old), _) if f.status == "renamed" && !old.is_empty() => {
            format!("renamed from {}", one_line(old, 60))
        }
        (_, true) => "binary file".into(),
        _ => String::new(),
    };
    FileRow {
        name,
        dir,
        status: if f.status.is_empty() {
            "modified".into()
        } else {
            f.status.clone()
        },
        additions: if !binary && a > 0 {
            format!("+{}", thousands(a))
        } else {
            String::new()
        },
        deletions: if !binary && d > 0 {
            format!("-{}", thousands(d))
        } else {
            String::new()
        },
        note,
        bar: if binary { [Square::None; 5] } else { bar(a, d) },
    }
}

/// The card for a push the host asks about.
pub fn export_card_view(i: &ExportCardInput) -> ExportCardView {
    let opens = i.pr.is_some();
    let lower = if opens {
        "push a branch and open a pull request"
    } else {
        "push a branch"
    };
    let who = if i.agent.trim().is_empty() {
        "An agent"
    } else {
        i.agent.trim()
    };
    let more = |n: u64| format!("+{} more", thousands(n));
    let files_total = i.files_total.max(i.files.len() as u64);
    let rows: Vec<FileRow> = i.files.iter().take(SHOW_FILES).map(file_row).collect();
    let more_files =
        (files_total > rows.len() as u64).then(|| more(files_total - rows.len() as u64));
    let commits_total = i.commits_total.max(i.commits.len() as u64);
    let commits: Vec<CommitRow> = i
        .commits
        .iter()
        .take(SHOW_COMMITS)
        .map(|c| CommitRow {
            sha: c.sha.chars().take(7).collect(),
            author: one_line(&c.author, 40),
            subject: one_line(&c.subject, SUBJECT_MAX),
        })
        .collect();
    let more_commits =
        (commits_total > commits.len() as u64).then(|| more(commits_total - commits.len() as u64));
    let base = i.base.clone().filter(|b| !b.trim().is_empty());
    let branch_line = match (&base, opens) {
        (Some(b), _) => format!("{} into {b}", i.branch),
        (None, true) => format!("{} into the default branch", i.branch),
        (None, false) => i.branch.clone(),
    };
    // A pull request preview only when one will really be opened and has a title.
    let pr =
        i.pr.as_ref()
            .filter(|p| !p.title.trim().is_empty())
            .map(|p| PrView {
                title: one_line(&p.title, TITLE_MAX),
                state: if p.draft { "Draft" } else { "Open" }.into(),
                body: p
                    .body
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| one_line(l, BODY_LINE_MAX))
                    .take(BODY_LINES)
                    .collect(),
            });
    let account = i
        .account
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(|a| format!("Cua pushes it from this computer as {a}."));
    let what = if opens {
        "push and pull request"
    } else {
        "push"
    };
    ExportCardView {
        repo: i.repo.clone(),
        headline: format!("{who} wants to {lower} on {}.", i.repo),
        branch: i.branch.clone(),
        base,
        branch_line,
        totals: Totals {
            files: if files_total == 0 {
                "No files".into()
            } else {
                plural(files_total, "file", "files")
            },
            additions: if i.additions_total > 0 {
                format!("+{}", thousands(i.additions_total))
            } else {
                String::new()
            },
            deletions: if i.deletions_total > 0 {
                format!("-{}", thousands(i.deletions_total))
            } else {
                String::new()
            },
        },
        account,
        expandable: !rows.is_empty() || !commits.is_empty() || pr.is_some(),
        show_label: "Show changes".into(),
        hide_label: "Hide changes".into(),
        files: rows,
        more_files,
        commits_heading: plural(commits_total, "commit", "commits"),
        commits,
        more_commits,
        pr,
        allow: if opens {
            "Push and open PR".into()
        } else {
            "Push".into()
        },
        deny: "Not now".into(),
        result_approved: format!("Approved the {what} of {} to {}", i.branch, i.repo),
        result_declined: format!("Declined the {what} of {} to {}", i.branch, i.repo),
    }
}

/// Who the thread shows for an author.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct AuthorInput {
    /// `person`, `agent` or `system`.
    pub kind: String,
    pub id: String,
    pub name: String,
}

/// How an author's name, tag and picture show.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorView {
    pub kind: String,
    pub name: String,
    /// The small tag next to the name (`APP`), for Cua's own entries only.
    pub tag: Option<String>,
    /// The picture: an id each shell maps to the app's own icon asset; `None` keeps the avatar the thread draws.
    pub avatar: Option<String>,
    /// The picture's accessibility label.
    pub avatar_label: Option<String>,
}

/// The asset id of the Cua Spaces app icon (the key on the app's icon).
pub const APP_ICON: &str = "cua-app-icon";

/// What the thread shows for an author. What Cua itself writes (the host's
/// cards and notices, `system:*`) shows as `Cua`, tagged `APP`, with the app
/// icon; agents and people keep how they show now.
pub fn author_view(a: &AuthorInput) -> AuthorView {
    if a.kind == "system" {
        return AuthorView {
            kind: "system".into(),
            name: "Cua".into(),
            tag: Some("APP".into()),
            avatar: Some(APP_ICON.into()),
            avatar_label: Some("Cua app".into()),
        };
    }
    AuthorView {
        kind: a.kind.clone(),
        name: if a.name.trim().is_empty() {
            "Someone".into()
        } else {
            a.name.clone()
        },
        tag: None,
        avatar: None,
        avatar_label: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, a: u64, d: u64) -> CardFile {
        CardFile {
            path: path.into(),
            status: "modified".into(),
            additions: Some(a),
            deletions: Some(d),
            ..Default::default()
        }
    }

    fn input() -> ExportCardInput {
        ExportCardInput {
            agent: "Hermes".into(),
            repo: "acme/app".into(),
            branch: "fix/retry-backoff".into(),
            base: Some("main".into()),
            head: "0a1b2c3d4e5f".into(),
            files: vec![
                file("src/net/retry.py", 9, 3),
                file("tests/test_retry.py", 3, 0),
            ],
            files_total: 2,
            additions_total: 12,
            deletions_total: 3,
            commits: vec![CardCommit {
                sha: "0a1b2c3d4e5f".into(),
                author: "Hermes".into(),
                subject: "Back off between retries".into(),
            }],
            commits_total: 1,
            pr: Some(CardPr {
                title: "Back off between retries".into(),
                body: "Retries now wait 1s, 2s, 4s.\n\nFixes the flaky sync test.\nA third line."
                    .into(),
                draft: false,
            }),
            account: Some("ada, signed in with gh".into()),
            from: "release-checks".into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_card_says_it_in_plain_words() {
        let v = export_card_view(&input());
        assert_eq!(
            v.headline,
            "Hermes wants to push a branch and open a pull request on acme/app."
        );
        assert_eq!(v.branch_line, "fix/retry-backoff into main");
        assert_eq!(
            (
                v.totals.files.as_str(),
                v.totals.additions.as_str(),
                v.totals.deletions.as_str()
            ),
            ("2 files", "+12", "-3")
        );
        assert_eq!(v.files[0].name, "retry.py");
        assert_eq!(v.files[0].dir, "src/net/");
        assert_eq!(v.commits[0].sha, "0a1b2c3");
        assert_eq!(
            v.account.as_deref(),
            Some("Cua pushes it from this computer as ada, signed in with gh.")
        );
        assert_eq!(
            v.result_approved,
            "Approved the push and pull request of fix/retry-backoff to acme/app"
        );
        assert_eq!(
            v.pr.as_ref().unwrap().body,
            vec!["Retries now wait 1s, 2s, 4s.", "Fixes the flaky sync test."]
        );
    }

    #[test]
    fn the_bar_is_in_proportion_and_a_changed_side_never_vanishes() {
        use Square::{Add as A, Del as D, None as N};
        assert_eq!(bar(0, 0), [N; 5]);
        assert_eq!(bar(10, 0), [A; 5]);
        assert_eq!(bar(0, 7), [D; 5]);
        assert_eq!(bar(3, 3), [A, A, D, D, D]);
        assert_eq!(bar(1000, 1), [A, A, A, A, D]);
        assert_eq!(bar(1, 1000), [A, D, D, D, D]);
        assert_eq!(bar(1, 0), [A; 5]);
        for (a, d) in [(0, 1), (5, 5), (u64::MAX, u64::MAX), (u64::MAX, 1), (2, 9)] {
            let b = bar(a, d);
            assert_eq!(b.len(), 5);
            if a > 0 {
                assert!(b.contains(&A));
            }
            if d > 0 {
                assert!(b.contains(&D));
            }
        }
    }

    #[test]
    fn binary_files_renames_and_empty_diffs() {
        let mut i = input();
        i.files = vec![
            CardFile {
                path: "img/logo.png".into(),
                status: "modified".into(),
                binary: true,
                ..Default::default()
            },
            CardFile {
                path: "src/new_name.py".into(),
                old_path: Some("src/old_name.py".into()),
                status: "renamed".into(),
                additions: Some(0),
                deletions: Some(0),
                ..Default::default()
            },
        ];
        let v = export_card_view(&i);
        assert_eq!(v.files[0].note, "binary file");
        assert_eq!(
            (v.files[0].additions.as_str(), v.files[0].bar),
            ("", [Square::None; 5])
        );
        assert_eq!(v.files[1].note, "renamed from src/old_name.py");
        let mut z = input();
        z.files.clear();
        z.files_total = 0;
        z.additions_total = 0;
        z.deletions_total = 0;
        z.commits.clear();
        z.commits_total = 0;
        z.pr = None;
        let v = export_card_view(&z);
        assert_eq!(v.totals.files, "No files");
        assert_eq!(
            (v.totals.additions.as_str(), v.totals.deletions.as_str()),
            ("", "")
        );
        assert_eq!(v.allow, "Push");
    }

    #[test]
    fn a_very_large_diff_is_cut_and_still_counts_everything() {
        let mut i = input();
        i.files = (0..65)
            .map(|n| file(&format!("pkg/f{n}.rs"), 1000, 20))
            .collect();
        i.files_total = 12_345;
        i.additions_total = 4_000_000;
        i.deletions_total = 99_000;
        i.commits = (0..30)
            .map(|n| CardCommit {
                sha: format!("{n:040}"),
                author: "a".into(),
                subject: "x".repeat(300),
            })
            .collect();
        i.commits_total = 31;
        let v = export_card_view(&i);
        assert_eq!(v.files.len(), SHOW_FILES);
        assert_eq!(v.more_files.as_deref(), Some("+12,342 more"));
        assert_eq!(v.totals.files, "12,345 files");
        assert_eq!(
            (v.totals.additions.as_str(), v.totals.deletions.as_str()),
            ("+4,000,000", "-99,000")
        );
        assert_eq!(v.commits.len(), SHOW_COMMITS);
        assert_eq!(v.more_commits.as_deref(), Some("+28 more"));
        assert!(v.commits[0].subject.chars().count() <= SUBJECT_MAX);
    }

    #[test]
    fn the_buttons_say_what_will_happen() {
        let v = export_card_view(&input());
        assert_eq!(
            (v.allow.as_str(), v.deny.as_str()),
            ("Push and open PR", "Not now")
        );
        let mut push_only = input();
        push_only.pr = None;
        let v = export_card_view(&push_only);
        assert_eq!((v.allow.as_str(), v.deny.as_str()), ("Push", "Not now"));
        assert!(v.pr.is_none(), "no pull request, no preview");
        assert_eq!(v.headline, "Hermes wants to push a branch on acme/app.");
    }

    #[test]
    fn the_pull_request_preview_is_a_small_header_only_when_one_will_open() {
        let v = export_card_view(&input());
        let pr = v.pr.unwrap();
        assert_eq!(pr.title, "Back off between retries");
        assert_eq!(pr.state, "Open");
        assert_eq!(
            pr.body.len(),
            BODY_LINES,
            "at most two lines of the description"
        );
        let mut d = input();
        d.pr.as_mut().unwrap().draft = true;
        assert_eq!(export_card_view(&d).pr.unwrap().state, "Draft");
        let mut untitled = input();
        untitled.pr.as_mut().unwrap().title = "  ".into();
        assert!(
            export_card_view(&untitled).pr.is_none(),
            "no title, no preview"
        );
    }

    #[test]
    fn the_collapsed_card_is_a_few_lines_and_says_nothing_twice() {
        // Headline, branch line, diff line, account, Show changes, buttons.
        let v = export_card_view(&input());
        let lines = collapsed_lines(&v);
        assert!(lines.len() <= 6, "{lines:?}");
        assert_eq!(lines[2], "2 files +12 -3");
        // Even with every optional piece, the collapsed card does not grow.
        let mut big = input();
        big.files = (0..65)
            .map(|n| file(&format!("pkg/f{n}.rs"), 1, 1))
            .collect();
        big.files_total = 9000;
        big.commits = (0..30)
            .map(|n| CardCommit {
                sha: format!("{n:040}"),
                author: "a".into(),
                subject: "s".into(),
            })
            .collect();
        big.commits_total = 300;
        assert!(collapsed_lines(&export_card_view(&big)).len() <= 6);
        for l in collapsed_lines(&v) {
            assert!(l.chars().count() <= 110 && !l.contains('\n'), "{l}");
        }
        // The files and commits are behind the expander, three of each.
        assert!(v.expandable && v.show_label == "Show changes" && v.hide_label == "Hide changes");
        let all = export_card_view(&big);
        assert_eq!(
            (all.files.len(), all.commits.len()),
            (SHOW_FILES, SHOW_COMMITS)
        );
        // Nothing to open: no expander.
        let mut empty = input();
        empty.files.clear();
        empty.files_total = 0;
        empty.commits.clear();
        empty.commits_total = 0;
        empty.pr = None;
        assert!(!export_card_view(&empty).expandable);
    }

    #[test]
    fn nothing_on_a_card_has_a_middle_dot_or_the_word_system() {
        let v = serde_json::to_string(&export_card_view(&input())).unwrap();
        assert!(
            !v.contains('\u{b7}') && !v.contains("SYSTEM") && !v.contains("System"),
            "{v}"
        );
        let a = serde_json::to_string(&author_view(&AuthorInput {
            kind: "system".into(),
            id: "system:host".into(),
            name: "Cua".into(),
        }))
        .unwrap();
        assert!(
            !a.contains('\u{b7}') && !a.contains("SYSTEM") && !a.contains("System"),
            "{a}"
        );
    }

    #[test]
    fn cuas_own_entries_are_tagged_app_with_the_app_icon() {
        let s = author_view(&AuthorInput {
            kind: "system".into(),
            id: "system:host".into(),
            name: "host".into(),
        });
        assert_eq!(
            (s.name.as_str(), s.tag.as_deref(), s.avatar.as_deref()),
            ("Cua", Some("APP"), Some(APP_ICON))
        );
        for kind in ["agent", "person"] {
            let v = author_view(&AuthorInput {
                kind: kind.into(),
                id: "x".into(),
                name: "Grace".into(),
            });
            assert_eq!((v.tag, v.avatar), (None, None));
        }
    }
}
