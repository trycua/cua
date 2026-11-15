// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Reviewing what an agent changed: the file list, one file's diff, and the
//! preview of a file.
//!
//! The daemon sends the facts (`space_changes`: the files with their counts;
//! `space_file_diff`: one file's `git diff` text; `space_file_preview`: the
//! bytes of an image or a text file). Everything a person reads is made here,
//! so the card's "Show changes", the file list and the full diff agree and the
//! shells only lay it out:
//!
//! * [`changes_view`]: `5 files +32 -19`, a row per file with the same
//!   five-square bar as the approval card ([`super::bars::bar`]);
//! * [`file_diff`]: the unified diff parsed into hunks and lines with both line
//!   numbers, and, for a removed line the next lines replace, the words that
//!   changed ([`Segment`]), so the shell can show a unified or a word view;
//! * [`preview_view`]: whether a file can be previewed (an image, or text),
//!   and if not, why.

use serde::{Deserialize, Serialize};

use super::bars::{Square, bar, thousands};

/// The most a text file previews at, in bytes.
pub const MAX_TEXT_BYTES: u64 = 512 * 1024;
/// The most an image previews at, in bytes.
pub const MAX_IMAGE_BYTES: u64 = 8 * 1024 * 1024;
/// A line longer than this gets no word diff.
const WORD_LINE_MAX: usize = 500;
/// Token pairs (old times new) a word diff may compare.
const WORD_CELLS_MAX: usize = 40_000;

/// The words around the diff screens (titles, buttons, the states while it loads or cannot).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Labels {
    /// The button on a push card and in the thread that opens the changes.
    pub open: String,
    pub title: String,
    pub diff_tab: String,
    pub preview_tab: String,
    pub loading: String,
    pub loading_file: String,
    /// The daemon cannot list changes for this Space.
    pub unavailable: String,
    pub retry: String,
    pub back: String,
    pub image_alt: String,
}

pub fn labels() -> Labels {
    Labels {
        open: "Open the full diff".into(),
        title: "Changes".into(),
        diff_tab: "Diff".into(),
        preview_tab: "Preview".into(),
        loading: "Loading changes\u{2026}".into(),
        loading_file: "Loading\u{2026}".into(),
        unavailable: "This Space cannot list its changes from the phone yet.".into(),
        retry: "Try again".into(),
        back: "Back to changes".into(),
        image_alt: "Preview of the image".into(),
    }
}

/* ---- The file list --------------------------------------------------------------- */

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FileChange {
    pub path: String,
    pub old_path: Option<String>,
    /// `added`, `modified`, `deleted`, `renamed`.
    pub status: String,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
    pub binary: bool,
    pub size: Option<u64>,
    pub mime: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChangesInput {
    pub repo: String,
    pub branch: String,
    pub base: Option<String>,
    pub head_sha: Option<String>,
    pub files: Vec<FileChange>,
    pub truncated: bool,
    /// The machine computed these from content it checked against the push approval's card (the daemon's `verified`).
    pub verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileItem {
    pub path: String,
    pub name: String,
    /// The folder, `src/app/`, or empty.
    pub dir: String,
    pub status: String,
    pub status_label: String,
    /// `+10`, or empty.
    pub additions: String,
    /// `-3`, or empty.
    pub deletions: String,
    pub note: String,
    pub bar: [Square; 5],
    pub binary: bool,
    /// `image`, `text` or `none` (what [`preview_view`] says).
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesView {
    pub title: String,
    /// `acme/app`.
    pub repo: String,
    /// `fix/retry into main`.
    pub branch_line: String,
    /// `5 files`, `+32`, `-19`.
    pub files_total: String,
    pub additions: String,
    pub deletions: String,
    pub files: Vec<FileItem>,
    pub truncated_note: Option<String>,
    pub empty: Option<String>,
    /// "Checked against what the push approval shows", when the machine checked it.
    pub verified_note: Option<String>,
}

fn split_path(p: &str) -> (String, String) {
    match p.rsplit_once('/') {
        Some((dir, name)) if !name.is_empty() => (name.to_string(), format!("{dir}/")),
        _ => (p.to_string(), String::new()),
    }
}

fn status_label(s: &str) -> &'static str {
    match s {
        "added" => "Added",
        "deleted" => "Deleted",
        "renamed" => "Renamed",
        _ => "Modified",
    }
}

/// The file list for `i`.
pub fn changes_view(i: &ChangesInput) -> ChangesView {
    let mut adds = 0u64;
    let mut dels = 0u64;
    let files: Vec<FileItem> = i
        .files
        .iter()
        .map(|f| {
            let binary = f.binary || (f.additions.is_none() && f.deletions.is_none());
            let (a, d) = (f.additions.unwrap_or(0), f.deletions.unwrap_or(0));
            if !binary {
                adds += a;
                dels += d;
            }
            let (name, dir) = split_path(&f.path);
            let note = match (&f.old_path, binary) {
                (Some(old), _) if f.status == "renamed" && !old.is_empty() => {
                    format!("renamed from {old}")
                }
                (_, true) => "binary file".into(),
                _ => String::new(),
            };
            let status = if f.status.is_empty() {
                "modified".to_string()
            } else {
                f.status.clone()
            };
            FileItem {
                preview: preview_kind(&f.path, f.mime.as_deref(), f.size, binary)
                    .0
                    .into(),
                status_label: status_label(&status).into(),
                status,
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
                bar: if binary { [Square::None; 5] } else { bar(a, d) },
                path: f.path.clone(),
                name,
                dir,
                note,
                binary,
            }
        })
        .collect();
    let n = files.len() as u64;
    let branch_line = match (
        i.branch.is_empty(),
        i.base.as_deref().filter(|b| !b.is_empty()),
    ) {
        (true, _) => String::new(),
        (false, Some(base)) => format!("{} into {base}", i.branch),
        (false, None) => i.branch.clone(),
    };
    ChangesView {
        title: "Changes".into(),
        repo: i.repo.clone(),
        branch_line,
        files_total: format!("{} {}", thousands(n), if n == 1 { "file" } else { "files" }),
        additions: if adds > 0 {
            format!("+{}", thousands(adds))
        } else {
            String::new()
        },
        deletions: if dels > 0 {
            format!("-{}", thousands(dels))
        } else {
            String::new()
        },
        truncated_note: i
            .truncated
            .then(|| "Only the first files are listed.".to_string()),
        empty: files.is_empty().then(|| "No changes yet.".to_string()),
        verified_note: i
            .verified
            .then(|| "Checked against what the push approval shows.".to_string()),
        files,
    }
}

/* ---- One file's diff -------------------------------------------------------------- */

/// A run of a line's text, changed or not (only on lines a word diff covers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Segment {
    pub text: String,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LineView {
    /// `context`, `add`, `del`.
    pub kind: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
    /// Empty unless the line pairs with one on the other side.
    pub segments: Vec<Segment>,
    /// The file has no newline at the end after this line.
    pub no_newline: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HunkView {
    /// `@@ -1,4 +1,6 @@ fn name`.
    pub header: String,
    pub lines: Vec<LineView>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DiffInput {
    pub path: String,
    pub patch: String,
    pub truncated: bool,
    pub binary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiffView {
    pub path: String,
    pub name: String,
    pub dir: String,
    pub hunks: Vec<HunkView>,
    pub additions: String,
    pub deletions: String,
    /// Lines the reader should know (cut short, binary, nothing to show).
    pub notes: Vec<String>,
    pub unified_label: String,
    pub words_label: String,
    /// Whether any line has word-level segments (the toggle is offered only then).
    pub has_words: bool,
}

fn hunk_start(rest: &str, sign: char) -> Option<u32> {
    let rest = rest.strip_prefix(sign)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// `(old start, new start)` of an `@@ -a,b +c,d @@` header.
fn parse_header(line: &str) -> Option<(u32, u32)> {
    let rest = line.strip_prefix("@@ ")?;
    let mut it = rest.split_whitespace();
    let old = hunk_start(it.next()?, '-')?;
    let new = hunk_start(it.next()?, '+')?;
    Some((old, new))
}

fn class(c: char) -> u8 {
    if c.is_alphanumeric() || c == '_' {
        0
    } else if c.is_whitespace() {
        1
    } else {
        2
    }
}

fn tokens(s: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    let mut prev: Option<u8> = None;
    for (i, c) in s.char_indices() {
        let k = class(c);
        if let Some(p) = prev
            && (p != k || k == 2)
        {
            out.push(&s[start..i]);
            start = i;
        }
        prev = Some(k);
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

fn merge(parts: Vec<(String, bool)>) -> Vec<Segment> {
    let mut out: Vec<Segment> = vec![];
    for (text, changed) in parts {
        match out.last_mut() {
            Some(l) if l.changed == changed => l.text.push_str(&text),
            _ => out.push(Segment { text, changed }),
        }
    }
    out
}

/// The word-level segments of an old and a new line, or none when the lines
/// share too little for highlighting to help (the whole line then reads as changed).
pub fn word_segments(old: &str, new: &str) -> Option<(Vec<Segment>, Vec<Segment>)> {
    if old.len() > WORD_LINE_MAX || new.len() > WORD_LINE_MAX {
        return None;
    }
    let (a, b) = (tokens(old), tokens(new));
    if a.is_empty() || b.is_empty() || a.len() * b.len() > WORD_CELLS_MAX {
        return None;
    }
    // Longest common subsequence of tokens.
    let (n, m) = (a.len(), b.len());
    let mut t = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] {
                t[i + 1][j + 1] + 1
            } else {
                t[i + 1][j].max(t[i][j + 1])
            };
        }
    }
    let (mut ka, mut kb) = (vec![false; n], vec![false; m]);
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            ka[i] = true;
            kb[j] = true;
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let shared: usize = a
        .iter()
        .zip(&ka)
        .filter(|(tok, k)| **k && !tok.trim().is_empty())
        .map(|(tok, _)| tok.chars().count())
        .sum();
    let longest = old.chars().count().max(new.chars().count()).max(1);
    // Under a third in common: the lines are different lines.
    if shared * 3 < longest {
        return None;
    }
    let side = |toks: &[&str], keep: &[bool]| {
        merge(
            toks.iter()
                .zip(keep)
                .map(|(t, k)| ((*t).to_string(), !*k))
                .collect(),
        )
    };
    Some((side(&a, &ka), side(&b, &kb)))
}

/// One file's diff as hunks of numbered lines, with word segments on paired lines.
pub fn file_diff(i: &DiffInput) -> FileDiffView {
    let (name, dir) = split_path(&i.path);
    let mut hunks: Vec<HunkView> = vec![];
    let mut in_header = true;
    let mut old_no = 0u32;
    let mut new_no = 0u32;
    for raw in i.patch.lines() {
        if raw.starts_with("diff --git ") {
            in_header = true;
            continue;
        }
        if raw.starts_with("@@") {
            if let Some((o, n)) = parse_header(raw) {
                in_header = false;
                old_no = o;
                new_no = n;
                hunks.push(HunkView {
                    header: raw.trim_end().to_string(),
                    lines: vec![],
                });
            }
            continue;
        }
        if in_header {
            continue;
        }
        let Some(h) = hunks.last_mut() else { continue };
        let (kind, text) = match raw.chars().next() {
            Some('+') => ("add", &raw[1..]),
            Some('-') => ("del", &raw[1..]),
            Some(' ') => ("context", &raw[1..]),
            Some('\\') => {
                if let Some(l) = h.lines.last_mut() {
                    l.no_newline = true;
                }
                continue;
            }
            // A blank context line some tools trim to nothing.
            None => ("context", ""),
            _ => continue,
        };
        let (o, n) = match kind {
            "add" => (None, Some(new_no)),
            "del" => (Some(old_no), None),
            _ => (Some(old_no), Some(new_no)),
        };
        if kind != "add" {
            old_no += 1;
        }
        if kind != "del" {
            new_no += 1;
        }
        h.lines.push(LineView {
            kind: kind.into(),
            old_no: o,
            new_no: n,
            text: text.to_string(),
            segments: vec![],
            no_newline: false,
        });
    }
    // Pair each run of removed lines with the run of added lines right after it.
    let mut has_words = false;
    for h in &mut hunks {
        let mut k = 0;
        while k < h.lines.len() {
            if h.lines[k].kind != "del" {
                k += 1;
                continue;
            }
            let ds = k;
            while k < h.lines.len() && h.lines[k].kind == "del" {
                k += 1;
            }
            let as_ = k;
            while k < h.lines.len() && h.lines[k].kind == "add" {
                k += 1;
            }
            for p in 0..(as_ - ds).min(k - as_) {
                let (old, new) = (h.lines[ds + p].text.clone(), h.lines[as_ + p].text.clone());
                if let Some((so, sn)) = word_segments(&old, &new) {
                    h.lines[ds + p].segments = so;
                    h.lines[as_ + p].segments = sn;
                    has_words = true;
                }
            }
        }
    }
    let count = |kind: &str| {
        hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == kind)
            .count() as u64
    };
    let (a, d) = (count("add"), count("del"));
    let mut notes = vec![];
    if i.binary {
        notes.push("Binary file. There is no text diff to show.".to_string());
    } else if hunks.is_empty() {
        notes.push(
            if i.truncated {
                "This diff is too large to show here."
            } else {
                "No line changes in this file."
            }
            .to_string(),
        );
    } else if i.truncated {
        notes.push("This diff was cut short because it is large.".to_string());
    }
    FileDiffView {
        path: i.path.clone(),
        name,
        dir,
        hunks,
        additions: if a > 0 {
            format!("+{}", thousands(a))
        } else {
            String::new()
        },
        deletions: if d > 0 {
            format!("-{}", thousands(d))
        } else {
            String::new()
        },
        notes,
        unified_label: "Unified".into(),
        words_label: "Words".into(),
        has_words,
    }
}

/* ---- Previews --------------------------------------------------------------------- */

fn ext(path: &str) -> String {
    path.rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default()
}

/// `812 bytes`, `12.4 KB`, `3.0 MB`.
pub fn size_label(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} {}", bytes, if bytes == 1 { "byte" } else { "bytes" })
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// `(kind, why not)`: `image`, `text` or `none`.
fn preview_kind(
    path: &str,
    mime: Option<&str>,
    size: Option<u64>,
    binary: bool,
) -> (&'static str, Option<String>) {
    let mime = mime.unwrap_or("").to_ascii_lowercase();
    let e = ext(path);
    let image = mime.starts_with("image/") && mime != "image/svg+xml"
        || matches!(e.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic");
    if image {
        return match size {
            Some(s) if s > MAX_IMAGE_BYTES => (
                "none",
                Some(format!(
                    "This image is too large to preview here ({}).",
                    size_label(s)
                )),
            ),
            _ => ("image", None),
        };
    }
    let texty = mime.starts_with("text/")
        || matches!(
            mime.as_str(),
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-sh"
                | "application/yaml"
                | "application/toml"
                | "image/svg+xml"
        )
        || matches!(
            e.as_str(),
            "txt"
                | "md"
                | "json"
                | "yaml"
                | "yml"
                | "toml"
                | "rs"
                | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "mjs"
                | "py"
                | "go"
                | "swift"
                | "kt"
                | "java"
                | "c"
                | "h"
                | "cpp"
                | "hpp"
                | "cs"
                | "rb"
                | "php"
                | "sh"
                | "zsh"
                | "css"
                | "scss"
                | "html"
                | "xml"
                | "svg"
                | "sql"
                | "csv"
                | "lock"
                | "env"
                | "ini"
                | "cfg"
                | "conf"
                | "log"
                | "gitignore"
                | "dockerfile"
        );
    if binary || !texty {
        return (
            "none",
            Some("This kind of file can't be previewed here.".to_string()),
        );
    }
    match size {
        Some(s) if s > MAX_TEXT_BYTES => (
            "none",
            Some(format!(
                "This file is too large to preview here ({}).",
                size_label(s)
            )),
        ),
        _ => ("text", None),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PreviewInput {
    pub path: String,
    pub mime: Option<String>,
    pub size: Option<u64>,
    pub binary: bool,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewView {
    /// `image`, `text` or `none`.
    pub kind: String,
    pub name: String,
    pub size_label: Option<String>,
    /// Why there is nothing to show, or what the reader should know.
    pub note: Option<String>,
}

pub fn preview_view(i: &PreviewInput) -> PreviewView {
    let (kind, why) = preview_kind(&i.path, i.mime.as_deref(), i.size, i.binary);
    let note = why.or_else(|| {
        i.truncated
            .then(|| "Showing the start of this file.".to_string())
    });
    PreviewView {
        kind: kind.into(),
        name: split_path(&i.path).0,
        size_label: i.size.map(size_label),
        note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "diff --git a/src/ws/client.ts b/src/ws/client.ts\nindex 1a2b3c4..5d6e7f8 100644\n--- a/src/ws/client.ts\n+++ b/src/ws/client.ts\n@@ -10,7 +10,8 @@ export class Client {\n   private retries = 0;\n-  const delay = base * attempt;\n+  const delay = Math.min(base * 2 ** attempt, cap);\n   await sleep(delay);\n-  // -- old note\n+  // new note\n+  log(delay);\n   return this;\n\\ No newline at end of file\n";

    fn change(path: &str, status: &str, a: Option<u64>, d: Option<u64>) -> FileChange {
        FileChange {
            path: path.into(),
            status: status.into(),
            additions: a,
            deletions: d,
            binary: a.is_none() && d.is_none(),
            ..Default::default()
        }
    }

    #[test]
    fn the_screens_have_their_words() {
        let l = labels();
        assert_eq!(
            (l.open.as_str(), l.title.as_str()),
            ("Open the full diff", "Changes")
        );
        assert!(l.unavailable.contains("cannot list its changes"));
    }

    #[test]
    fn the_file_list_counts_like_the_approval_card() {
        let v = changes_view(&ChangesInput {
            repo: "acme/checkout-api".into(),
            branch: "fix/retry".into(),
            base: Some("main".into()),
            files: vec![
                change("src/ws/client.ts", "modified", Some(12), Some(3)),
                change("README.md", "added", Some(20), Some(0)),
                FileChange {
                    path: "assets/logo.png".into(),
                    status: "modified".into(),
                    binary: true,
                    size: Some(2048),
                    mime: Some("image/png".into()),
                    ..Default::default()
                },
                FileChange {
                    path: "src/b.ts".into(),
                    old_path: Some("src/a.ts".into()),
                    status: "renamed".into(),
                    additions: Some(1),
                    deletions: Some(1),
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
        assert_eq!(v.branch_line, "fix/retry into main");
        assert_eq!(
            (
                v.files_total.as_str(),
                v.additions.as_str(),
                v.deletions.as_str()
            ),
            ("4 files", "+33", "-4")
        );
        assert_eq!(
            (v.files[0].name.as_str(), v.files[0].dir.as_str()),
            ("client.ts", "src/ws/")
        );
        assert_eq!(
            (v.files[0].additions.as_str(), v.files[0].deletions.as_str()),
            ("+12", "-3")
        );
        assert_eq!(v.files[0].bar, bar(12, 3));
        assert_eq!(v.files[1].status_label, "Added");
        assert_eq!(v.files[1].deletions, "");
        assert_eq!(
            (
                v.files[2].note.as_str(),
                v.files[2].preview.as_str(),
                v.files[2].binary
            ),
            ("binary file", "image", true)
        );
        assert_eq!(v.files[2].bar, [Square::None; 5]);
        assert_eq!(v.files[3].note, "renamed from src/a.ts");
        assert_eq!(v.empty, None);
        let one = changes_view(&ChangesInput {
            files: vec![change("a", "modified", Some(1), Some(0))],
            ..Default::default()
        });
        assert_eq!(one.files_total, "1 file");
        let none = changes_view(&ChangesInput::default());
        assert_eq!(none.empty.as_deref(), Some("No changes yet."));
        assert_eq!(none.branch_line, "");
        assert_eq!(none.verified_note, None);
        let checked = changes_view(&ChangesInput {
            verified: true,
            ..Default::default()
        });
        assert_eq!(
            checked.verified_note.as_deref(),
            Some("Checked against what the push approval shows.")
        );
    }

    #[test]
    fn a_unified_diff_becomes_numbered_hunks() {
        let v = file_diff(&DiffInput {
            path: "src/ws/client.ts".into(),
            patch: PATCH.into(),
            ..Default::default()
        });
        assert_eq!(v.hunks.len(), 1);
        let h = &v.hunks[0];
        assert_eq!(h.header, "@@ -10,7 +10,8 @@ export class Client {");
        let kinds: Vec<&str> = h.lines.iter().map(|l| l.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "context", "del", "add", "context", "del", "add", "add", "context"
            ]
        );
        assert_eq!((h.lines[0].old_no, h.lines[0].new_no), (Some(10), Some(10)));
        assert_eq!((h.lines[1].old_no, h.lines[1].new_no), (Some(11), None));
        assert_eq!((h.lines[2].old_no, h.lines[2].new_no), (None, Some(11)));
        assert_eq!((h.lines[3].old_no, h.lines[3].new_no), (Some(12), Some(12)));
        // A removed line that starts with dashes is a removal, not a file header.
        assert_eq!(h.lines[4].text, "  // -- old note");
        assert_eq!((v.additions.as_str(), v.deletions.as_str()), ("+3", "-2"));
        assert!(h.lines.last().unwrap().no_newline);
        assert_eq!((v.name.as_str(), v.dir.as_str()), ("client.ts", "src/ws/"));
        assert!(v.notes.is_empty());
    }

    #[test]
    fn paired_lines_get_word_segments_and_unpaired_ones_do_not() {
        let v = file_diff(&DiffInput {
            path: "a.ts".into(),
            patch: PATCH.into(),
            ..Default::default()
        });
        let h = &v.hunks[0];
        let old = &h.lines[1];
        let new = &h.lines[2];
        assert!(v.has_words);
        // Only what changed is marked; the shared words are not.
        let changed = |segs: &[Segment]| {
            segs.iter()
                .filter(|s| s.changed)
                .map(|s| s.text.clone())
                .collect::<String>()
        };
        assert_eq!(
            old.segments
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>(),
            old.text
        );
        assert_eq!(
            new.segments
                .iter()
                .map(|s| s.text.as_str())
                .collect::<String>(),
            new.text
        );
        assert!(changed(&new.segments).contains("Math.min"));
        assert!(!changed(&old.segments).contains("delay"));
        assert!(
            old.segments
                .iter()
                .any(|s| !s.changed && s.text.contains("const delay"))
        );
        // The third added line has nothing to pair with.
        assert!(h.lines[6].segments.is_empty());
        // Context lines never have segments.
        assert!(h.lines[0].segments.is_empty());
    }

    #[test]
    fn lines_that_share_nothing_are_not_highlighted() {
        assert!(word_segments("alpha beta gamma", "123 456 789").is_none());
        let (o, n) = word_segments("let x = 1;", "let x = 2;").unwrap();
        assert_eq!(
            o.iter()
                .filter(|s| s.changed)
                .map(|s| s.text.as_str())
                .collect::<String>(),
            "1"
        );
        assert_eq!(
            n.iter()
                .filter(|s| s.changed)
                .map(|s| s.text.as_str())
                .collect::<String>(),
            "2"
        );
        assert!(
            word_segments(&"a ".repeat(400), "b").is_none(),
            "long lines are not compared"
        );
    }

    #[test]
    fn empty_binary_and_cut_diffs_say_so() {
        let empty = file_diff(&DiffInput {
            path: "a".into(),
            ..Default::default()
        });
        assert_eq!(empty.notes, ["No line changes in this file."]);
        let bin = file_diff(&DiffInput {
            path: "a.png".into(),
            binary: true,
            ..Default::default()
        });
        assert_eq!(bin.notes, ["Binary file. There is no text diff to show."]);
        let big = file_diff(&DiffInput {
            path: "a".into(),
            truncated: true,
            ..Default::default()
        });
        assert_eq!(big.notes, ["This diff is too large to show here."]);
        let cut = file_diff(&DiffInput {
            path: "a".into(),
            patch: PATCH.into(),
            truncated: true,
            ..Default::default()
        });
        assert_eq!(cut.notes, ["This diff was cut short because it is large."]);
    }

    #[test]
    fn a_new_file_and_two_hunks_number_from_their_headers() {
        let patch = "diff --git a/n.txt b/n.txt\nnew file mode 100644\n--- /dev/null\n+++ b/n.txt\n@@ -0,0 +1,2 @@\n+one\n+two\n@@ -20,2 +22,2 @@\n x\n-y\n+z\n";
        let v = file_diff(&DiffInput {
            path: "n.txt".into(),
            patch: patch.into(),
            ..Default::default()
        });
        assert_eq!(v.hunks.len(), 2);
        assert_eq!(v.hunks[0].lines[1].new_no, Some(2));
        assert_eq!(v.hunks[1].lines[0].old_no, Some(20));
        assert_eq!(v.hunks[1].lines[0].new_no, Some(22));
        assert_eq!(v.hunks[1].lines[2].new_no, Some(23));
    }

    #[test]
    fn previews_are_for_images_and_text_and_say_why_not() {
        let p = |path: &str, mime: Option<&str>, size: Option<u64>, binary: bool| {
            preview_view(&PreviewInput {
                path: path.into(),
                mime: mime.map(str::to_string),
                size,
                binary,
                truncated: false,
            })
        };
        assert_eq!(
            p("a/logo.png", Some("image/png"), Some(2048), true).kind,
            "image"
        );
        assert_eq!(p("a/photo.JPG", None, None, true).kind, "image");
        assert_eq!(p("src/main.rs", None, Some(1200), false).kind, "text");
        assert_eq!(p("notes", Some("text/plain"), Some(10), false).kind, "text");
        let big = p(
            "data.json",
            Some("application/json"),
            Some(MAX_TEXT_BYTES + 1),
            false,
        );
        assert_eq!(big.kind, "none");
        assert_eq!(
            big.note.as_deref(),
            Some("This file is too large to preview here (512.0 KB).")
        );
        let bigimg = p("x.png", Some("image/png"), Some(MAX_IMAGE_BYTES + 1), true);
        assert_eq!(
            (bigimg.kind.as_str(), bigimg.note.is_some()),
            ("none", true)
        );
        let zip = p("a.zip", Some("application/zip"), Some(10), true);
        assert_eq!(
            zip.note.as_deref(),
            Some("This kind of file can't be previewed here.")
        );
        let cut = preview_view(&PreviewInput {
            path: "a.txt".into(),
            size: Some(5),
            truncated: true,
            ..Default::default()
        });
        assert_eq!(cut.note.as_deref(), Some("Showing the start of this file."));
        assert_eq!(size_label(1), "1 byte");
        assert_eq!(size_label(812), "812 bytes");
        assert_eq!(size_label(12_700), "12.4 KB");
        assert_eq!(size_label(3 * 1024 * 1024), "3.0 MB");
    }
}
