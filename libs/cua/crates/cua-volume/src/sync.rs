// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Copy-in and copy-out of a directory tree: how an agent home reaches the
//! directory its harness reads, and comes back.
//!
//! This is deliberately not a live filesystem: harnesses keep SQLite files
//! in their homes, and SQLite over a network filesystem corrupts. A home is
//! pulled before a run starts and pushed after each turn and on stop, by
//! one writer (see [`crate::lease`]). A manifest of SHA-256s
//! (`<prefix>.cua-manifest.json`) makes a push send only what changed.
//!
//! Every write goes through a [`Session`], so access rules and the secret
//! scanner apply: a file that trips the scanner is skipped and reported,
//! the rest of the push still lands.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::backend::Condition;
use crate::{Error, Mode, Result, Session, path, sha256_hex};

/// The manifest object's name inside a synced folder.
pub const MANIFEST_NAME: &str = ".cua-manifest.json";

/// Never synced: harness credentials, secrets, caches and noise. A pattern
/// ending in `/` matches a directory at any depth; one starting with `*`
/// matches a suffix; anything else matches a file name at any depth.
pub const DEFAULT_IGNORES: &[&str] = &[
    ".env",
    "auth.json",
    ".credentials.json",
    "credentials/",
    "vault/",
    "sessions/",
    "log/",
    "logs/",
    "cache/",
    ".cache/",
    "node_modules/",
    ".git/",
    "*.log",
    "*.sock",
];

/// Whether `rel` (a `/`-separated relative path) is excluded.
pub fn ignored(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    let name = parts.last().copied().unwrap_or("");
    if name.starts_with(crate::drive::INTERNAL_PREFIX) {
        return true;
    }
    DEFAULT_IGNORES.iter().any(|p| {
        if let Some(dir) = p.strip_suffix('/') {
            parts[..parts.len().saturating_sub(1)].contains(&dir)
        } else if let Some(suffix) = p.strip_prefix('*') {
            name.ends_with(suffix)
        } else {
            name == *p
        }
    })
}

/// One file's state in the manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileState {
    pub sha256: String,
    pub size: u64,
}

/// What a synced folder holds, by relative path.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub files: BTreeMap<String, FileState>,
}

/// What a push did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushReport {
    pub uploaded: Vec<String>,
    pub deleted: Vec<String>,
    pub unchanged: usize,
    /// Files the scanner kept out: `(path, kind)`.
    pub blocked: Vec<(String, String)>,
    pub bytes: u64,
}

/// What a pull fetched.
#[derive(Clone, Debug, Default)]
pub struct Pulled {
    /// `(relative path, bytes)` for every file that differs from `have`.
    pub files: Vec<(String, Vec<u8>)>,
    /// Relative paths in `have` the drive no longer holds.
    pub removed: Vec<String>,
    pub bytes: u64,
}

fn manifest_key(prefix: &str) -> String {
    format!("{prefix}{MANIFEST_NAME}")
}

/// The folder's manifest (empty when never pushed).
pub async fn manifest(session: &Session, prefix: &str) -> Result<Manifest> {
    let prefix = path::folder(prefix)?;
    if session.mode(&prefix)?.is_none() {
        return Err(Error::Forbidden(format!("cannot read {prefix}")));
    }
    match session
        .drive()
        .backend()
        .get(&manifest_key(&prefix), None)
        .await
    {
        Ok((b, _)) => Ok(serde_json::from_slice(&b)?),
        Err(Error::NotFound(_)) => Ok(Manifest::default()),
        Err(e) => Err(e),
    }
}

/// Uploads `files` (the changed ones) into `prefix`, deletes `deleted`, and
/// records `local` (every file the source holds, with hashes) as the new
/// manifest. Files the scanner blocks are left out of the manifest too, so
/// the next push tries them again.
pub async fn push(
    session: &Session,
    prefix: &str,
    local: &BTreeMap<String, FileState>,
    files: Vec<(String, Vec<u8>)>,
) -> Result<PushReport> {
    let prefix = path::folder(prefix)?;
    if session.mode(&prefix)? != Some(Mode::ReadWrite) {
        return Err(Error::Forbidden(format!(
            "{} may not write {prefix}",
            session.context().principal
        )));
    }
    let before = manifest(session, &prefix).await?;
    let mut report = PushReport::default();
    let mut recorded = local.clone();
    recorded.retain(|rel, _| !ignored(rel));
    let mut batch = vec![];
    for (rel, bytes) in files {
        if ignored(&rel) {
            continue;
        }
        batch.push((format!("{prefix}{}", path::normalize(&rel)?), bytes));
    }
    let (written, blocked) = session.write_many_quiet(batch).await?;
    for m in written {
        report.bytes += m.size;
        report.uploaded.push(m.key[prefix.len()..].to_string());
    }
    for (key, kind) in blocked {
        let rel = key[prefix.len()..].to_string();
        recorded.remove(&rel);
        report.blocked.push((rel, kind));
    }
    report.uploaded.sort();
    for rel in before.files.keys() {
        if !recorded.contains_key(rel) && !report.blocked.iter().any(|(r, _)| r == rel) {
            let key = format!("{prefix}{rel}");
            match session.delete(&key, Condition::None).await {
                Ok(()) | Err(Error::NotFound(_)) => report.deleted.push(rel.clone()),
                Err(e) => return Err(e),
            }
        }
    }
    report.unchanged = recorded.len().saturating_sub(report.uploaded.len());
    let m = Manifest { files: recorded };
    session
        .drive()
        .backend()
        .put(
            &manifest_key(&prefix),
            serde_json::to_vec(&m)?,
            Condition::None,
        )
        .await?;
    let _ = session.drive().audit().append(
        &session.context().principal.id(),
        "sync_out",
        &prefix,
        &format!(
            "{} up, {} deleted, {} unchanged, {} blocked, {} bytes",
            report.uploaded.len(),
            report.deleted.len(),
            report.unchanged,
            report.blocked.len(),
            report.bytes
        ),
    );
    Ok(report)
}

/// Which files of `local` differ from the drive's manifest (the files to
/// send on a push).
pub fn changed(local: &BTreeMap<String, FileState>, remote: &Manifest) -> Vec<String> {
    local
        .iter()
        .filter(|(rel, s)| !ignored(rel) && remote.files.get(*rel) != Some(*s))
        .map(|(rel, _)| rel.clone())
        .collect()
}

/// Fetches every file under `prefix` that differs from `have` (what the
/// destination already holds, by hash).
pub async fn pull(
    session: &Session,
    prefix: &str,
    have: &BTreeMap<String, FileState>,
) -> Result<Pulled> {
    let prefix = path::folder(prefix)?;
    let mut out = Pulled::default();
    let objects = match session.walk(&prefix).await {
        Ok(o) => o,
        Err(Error::NotFound(_)) => vec![],
        Err(e) => return Err(e),
    };
    let m = manifest(session, &prefix).await?;
    let mut present = std::collections::BTreeSet::new();
    for o in objects {
        let rel = o.key[prefix.len()..].to_string();
        if ignored(&rel) {
            continue;
        }
        present.insert(rel.clone());
        let known = m.files.get(&rel).map(|s| s.sha256.as_str());
        if known.is_some() && have.get(&rel).map(|s| s.sha256.as_str()) == known {
            continue;
        }
        let (bytes, _) = session.read_logged(&o.key, None, false).await?;
        if have
            .get(&rel)
            .is_some_and(|s| s.sha256 == sha256_hex(&bytes))
        {
            continue;
        }
        out.bytes += bytes.len() as u64;
        out.files.push((rel, bytes));
    }
    out.removed = have
        .keys()
        .filter(|r| !present.contains(*r) && !ignored(r))
        .cloned()
        .collect();
    let _ = session.drive().audit().append(
        &session.context().principal.id(),
        "sync_in",
        &prefix,
        &format!("{} files, {} bytes", out.files.len(), out.bytes),
    );
    Ok(out)
}

/// Hashes every file under a local directory (relative `/` paths),
/// skipping ignored ones. Bounded by the tree.
pub fn scan_dir(dir: &Path) -> Result<BTreeMap<String, FileState>> {
    let mut out = BTreeMap::new();
    if !dir.exists() {
        return Ok(out);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            let p = entry.path();
            let rel = p
                .strip_prefix(dir)
                .map_err(|e| Error::Backend(e.to_string()))?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if ft.is_dir() {
                if !ignored(&format!("{rel}/x")) {
                    stack.push(p);
                }
            } else if ft.is_file() && !ignored(&rel) {
                let bytes = std::fs::read(&p)?;
                out.insert(
                    rel,
                    FileState {
                        sha256: sha256_hex(&bytes),
                        size: bytes.len() as u64,
                    },
                );
            }
        }
    }
    Ok(out)
}

/// Pushes a local directory into `prefix` (the whole flow for callers that
/// hold the tree on disk).
pub async fn push_dir(session: &Session, prefix: &str, dir: &Path) -> Result<PushReport> {
    let local = scan_dir(dir)?;
    let remote = manifest(session, prefix).await?;
    let mut files = vec![];
    for rel in changed(&local, &remote) {
        files.push((rel.clone(), std::fs::read(dir.join(&rel))?));
    }
    push(session, prefix, &local, files).await
}

/// Pulls `prefix` into a local directory: writes what differs and removes
/// what the drive no longer holds.
pub async fn pull_dir(session: &Session, prefix: &str, dir: &Path) -> Result<Pulled> {
    let have = scan_dir(dir)?;
    let pulled = pull(session, prefix, &have).await?;
    for (rel, bytes) in &pulled.files {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, bytes)?;
    }
    for rel in &pulled.removed {
        let _ = std::fs::remove_file(dir.join(rel));
    }
    Ok(pulled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Drive};

    #[test]
    fn ignore_list() {
        for yes in [
            ".env",
            "hermes/.env",
            "codex/auth.json",
            "codex/sessions/a.jsonl",
            "x/node_modules/y.js",
            "a.log",
            "openclaw/credentials/k",
            ".cua-lease",
        ] {
            assert!(ignored(yes), "{yes}");
        }
        for no in [
            "hermes/memories/MEMORY.md",
            "codex/config.toml",
            "state.db",
            "envelope.txt",
        ] {
            assert!(!ignored(no), "{no}");
        }
    }

    #[tokio::test]
    async fn round_trip_sends_only_changes_and_blocks_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let d = Drive::open_local(&dir.path().join("home"));
        let ada = d.session(Context::agent("ada", Some("local:w")));
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("hermes/memories")).unwrap();
        std::fs::write(src.join("hermes/memories/MEMORY.md"), "likes tea").unwrap();
        std::fs::write(src.join("hermes/state.db"), vec![7u8; 4096]).unwrap();
        std::fs::write(src.join("hermes/.env"), "OPENAI_API_KEY=x").unwrap();
        let r = push_dir(&ada, "agents/ada/", &src).await.unwrap();
        assert_eq!(r.uploaded.len(), 2, "{r:?}");
        let r = push_dir(&ada, "agents/ada/", &src).await.unwrap();
        assert!(r.uploaded.is_empty() && r.unchanged == 2, "{r:?}");
        std::fs::write(src.join("hermes/memories/MEMORY.md"), "likes tea and jazz").unwrap();
        std::fs::remove_file(src.join("hermes/state.db")).unwrap();
        let leak = format!("token {}{}", "ghp_", "a".repeat(36));
        std::fs::write(src.join("hermes/memories/leak.md"), leak).unwrap();
        let r = push_dir(&ada, "agents/ada/", &src).await.unwrap();
        assert_eq!(r.uploaded, ["hermes/memories/MEMORY.md"]);
        assert_eq!(r.deleted, ["hermes/state.db"]);
        assert_eq!(
            r.blocked,
            [("hermes/memories/leak.md".into(), "GitHub token".into())]
        );
        // Restore into a fresh directory (a new Space).
        let dst = dir.path().join("dst");
        let p = pull_dir(&ada, "agents/ada/", &dst).await.unwrap();
        assert_eq!(p.files.len(), 1);
        assert_eq!(
            std::fs::read_to_string(dst.join("hermes/memories/MEMORY.md")).unwrap(),
            "likes tea and jazz"
        );
        assert!(!dst.join("hermes/.env").exists());
        // A second pull transfers nothing.
        assert!(
            pull_dir(&ada, "agents/ada/", &dst)
                .await
                .unwrap()
                .files
                .is_empty()
        );
        // Another agent cannot push into ada's home.
        let bob = d.session(Context::agent("bob", None));
        assert_eq!(
            push_dir(&bob, "agents/ada/", &src).await.unwrap_err().tag(),
            "forbidden"
        );
    }
}
