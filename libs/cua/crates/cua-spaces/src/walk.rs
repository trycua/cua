//! Host-side folder walks for transfers, optionally honoring ignore files.
//!
//! Semantics (ported from `walk_transfer_tree` in `spaces_mcp.py`, with the
//! matcher itself now the `ignore` crate's real gitignore implementation):
//!
//! - `.gitignore` and `.ignore` are read at the root and in every
//!   subdirectory, each scoped to its own subtree; a deeper file overrides a
//!   shallower one.
//! - `.dockerignore` is read at the root only (Docker's single root context).
//! - Within one file the last matching rule wins, so `!pattern` re-includes.
//! - An excluded directory is pruned: nothing beneath it is visited, so `!`
//!   cannot re-include below it (as in git).
//! - `.git/` is always skipped when ignore files are honored; symlinks are
//!   never followed or sent. Git's global excludes and `.git/info/exclude`
//!   are not consulted.

use crate::error::{Error, Result};
use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Ignore files honored in every directory.
pub const NESTED_IGNORE_FILES: [&str; 2] = [".gitignore", ".ignore"];
/// Ignore files honored at the transfer root only.
pub const ROOT_ONLY_IGNORE_FILES: [&str; 1] = [".dockerignore"];
/// Directories always skipped when ignore files are honored.
pub const ALWAYS_IGNORED_DIRS: [&str; 1] = [".git"];

/// Upper bound on entries in one transfer, so a mistaken `/` cannot turn into
/// an unbounded walk.
pub const MAX_ENTRIES: usize = 200_000;

/// One file or directory to transfer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalkEntry {
    /// Absolute host path.
    pub path: PathBuf,
    /// Path relative to the walk root, `/`-separated.
    pub rel: String,
    /// Directory (no content).
    pub is_dir: bool,
    /// File size.
    pub size: u64,
    /// Unix permission bits (0 when unknown).
    pub mode: u32,
    /// Modification time.
    pub modified: Option<SystemTime>,
}

/// The result of [`walk`].
#[derive(Clone, Debug, Default)]
pub struct Walk {
    /// Entries, sorted by `rel`; directories included so empty ones survive.
    pub entries: Vec<WalkEntry>,
    /// Paths left out by ignore rules (directories end in `/`), sorted.
    pub skipped: Vec<String>,
}

fn matcher(dir: &Path, names: &[&str]) -> Option<Gitignore> {
    let mut builder = GitignoreBuilder::new(dir);
    let mut any = false;
    for name in names {
        let path = dir.join(name);
        if path.is_file() {
            // A malformed line is skipped, not fatal (git does the same).
            let _ = builder.add(&path);
            any = true;
        }
    }
    if !any {
        return None;
    }
    builder.build().ok()
}

fn ignored(stack: &[Gitignore], path: &Path, is_dir: bool) -> bool {
    for m in stack.iter().rev() {
        match m.matched(path, is_dir) {
            Match::Ignore(_) => return true,
            Match::Whitelist(_) => return false,
            Match::None => {}
        }
    }
    false
}

fn entry(path: PathBuf, rel: String, meta: &std::fs::Metadata) -> WalkEntry {
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o7777
    };
    #[cfg(not(unix))]
    let mode = 0;
    WalkEntry {
        path,
        rel,
        is_dir: meta.is_dir(),
        size: if meta.is_dir() { 0 } else { meta.len() },
        mode,
        modified: meta.modified().ok(),
    }
}

/// Enumerates everything under `root` (not including `root` itself).
pub fn walk(root: &Path, respect_ignore_files: bool) -> Result<Walk> {
    let mut out = Walk::default();
    let mut root_names: Vec<&str> = NESTED_IGNORE_FILES.to_vec();
    root_names.extend(ROOT_ONLY_IGNORE_FILES);
    let base: Vec<Gitignore> = if respect_ignore_files {
        matcher(root, &root_names).into_iter().collect()
    } else {
        vec![]
    };
    let mut stack: Vec<(PathBuf, String, Vec<Gitignore>)> =
        vec![(root.to_path_buf(), String::new(), base)];
    while let Some((dir, rel_dir, matchers)) = stack.pop() {
        let mut children: Vec<_> = std::fs::read_dir(&dir)?.filter_map(|e| e.ok()).collect();
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().into_owned();
            let rel = if rel_dir.is_empty() {
                name.clone()
            } else {
                format!("{rel_dir}/{name}")
            };
            let path = child.path();
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() {
                continue;
            }
            let is_dir = meta.is_dir();
            if respect_ignore_files && is_dir && ALWAYS_IGNORED_DIRS.contains(&name.as_str()) {
                out.skipped.push(format!("{rel}/"));
                continue;
            }
            if respect_ignore_files && ignored(&matchers, &path, is_dir) {
                out.skipped.push(if is_dir {
                    format!("{rel}/")
                } else {
                    rel.clone()
                });
                continue;
            }
            out.entries.push(entry(path.clone(), rel.clone(), &meta));
            if out.entries.len() > MAX_ENTRIES {
                return Err(Error::Transfer(format!(
                    "{} holds more than {MAX_ENTRIES} entries; send a narrower folder",
                    root.display()
                )));
            }
            if is_dir {
                let mut nested = matchers.clone();
                if respect_ignore_files && let Some(m) = matcher(&path, &NESTED_IGNORE_FILES) {
                    nested.push(m);
                }
                stack.push((path, rel, nested));
            }
        }
    }
    out.entries.sort_by(|a, b| a.rel.cmp(&b.rel));
    out.skipped.sort();
    Ok(out)
}

/// A single file as a walk entry named `name`.
pub fn single(path: &Path, name: &str) -> Result<WalkEntry> {
    let meta = std::fs::metadata(path)?;
    Ok(entry(path.to_path_buf(), name.to_string(), &meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        dir
    }

    fn rels(w: &Walk) -> Vec<&str> {
        w.entries.iter().map(|e| e.rel.as_str()).collect()
    }

    // The same cases test_spaces_mcp.py pins for the Python matcher.
    #[test]
    fn nested_negation_and_directory_rules() {
        let dir = tree(&[
            (".gitignore", "*.log\nbuild/\n!keep.log\n"),
            ("a.log", ""),
            ("keep.log", ""),
            ("src/main.rs", ""),
            ("src/.gitignore", "*.tmp\n"),
            ("src/x.tmp", ""),
            ("build/out.bin", ""),
            ("node_modules/.keep", ""),
            (".git/HEAD", ""),
            ("docs/build", "a file named build is not a directory"),
        ]);
        let w = walk(dir.path(), true).unwrap();
        assert_eq!(
            rels(&w),
            vec![
                ".gitignore",
                "docs",
                "docs/build",
                "keep.log",
                "node_modules",
                "node_modules/.keep",
                "src",
                "src/.gitignore",
                "src/main.rs"
            ]
        );
        assert_eq!(w.skipped, vec![".git/", "a.log", "build/", "src/x.tmp"]);
    }

    #[test]
    fn dockerignore_is_root_only_and_anchoring_works() {
        let dir = tree(&[
            (".dockerignore", "secret.env\n"),
            ("secret.env", ""),
            ("sub/.dockerignore", "kept.txt\n"),
            ("sub/kept.txt", ""),
            ("sub/secret.env", ""),
            (".ignore", "/top.txt\n"),
            ("top.txt", ""),
            ("sub/top.txt", ""),
        ]);
        let w = walk(dir.path(), true).unwrap();
        let r = rels(&w);
        assert!(
            r.contains(&"sub/kept.txt"),
            "nested .dockerignore must not apply"
        );
        assert!(
            r.contains(&"sub/top.txt"),
            "anchored pattern only matches at root"
        );
        assert!(!r.contains(&"top.txt"));
        // A bare pattern matches at any depth.
        assert!(!r.contains(&"secret.env") && !r.contains(&"sub/secret.env"));
    }

    #[test]
    fn a_pruned_directory_cannot_be_reincluded_beneath() {
        let dir = tree(&[
            (".gitignore", "vendor/\n!vendor/keep.rs\n"),
            ("vendor/keep.rs", ""),
        ]);
        let w = walk(dir.path(), true).unwrap();
        assert!(!rels(&w).contains(&"vendor/keep.rs"));
    }

    #[test]
    fn respect_false_sends_everything_but_symlinks() {
        let dir = tree(&[(".gitignore", "*\n"), (".git/HEAD", ""), ("a", "")]);
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("link")).unwrap();
        let w = walk(dir.path(), false).unwrap();
        assert_eq!(rels(&w), vec![".git", ".git/HEAD", ".gitignore", "a"]);
        assert!(w.skipped.is_empty());
    }
}
