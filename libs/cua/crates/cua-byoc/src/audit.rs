// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `<cua home>/cloud/audit.jsonl` (0600): an append-only history of what
//! Cua did in your clouds. `state.json` holds only the present (a deleted
//! resource's record is dropped); this file keeps the past, one JSON
//! object per line:
//!
//! ```json
//! {"ts":"2026-10-21T09:00:00Z","action":"resource_deleted","provider":"aws",
//!  "kind":"instance","id":"i-0abc","space":"research","actor":"auto-sweep",
//!  "outcome":"ok","detail":"expired"}
//! ```
//!
//! Nothing secret is ever written: the fields are names and ids Cua
//! already shows in `cua cloud status`, and free text ([`redact`]) has
//! anything that looks like a token or key removed. At more than
//! [`ROTATE_BYTES`] the file moves to `audit.jsonl.1` (replacing the
//! previous one) and a new file starts, so the log never grows past about
//! twice that.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use cua_sandbox_core::byoc::CloudAuditEntry;

use crate::model;

/// The log rotates when it is larger than this.
pub const ROTATE_BYTES: u64 = 5 * 1024 * 1024;

/// The longest `detail` kept.
const DETAIL_MAX: usize = 300;

/// A new entry for `action` on `provider` by `actor`, outcome `ok`; the
/// rest is chained on.
pub fn entry(action: &str, provider: &str, actor: &str) -> CloudAuditEntry {
    CloudAuditEntry {
        ts: String::new(),
        action: action.into(),
        provider: provider.into(),
        actor: actor.into(),
        outcome: "ok".into(),
        ..Default::default()
    }
}

/// The audit entry of `r` for `action`.
pub fn of_resource(
    action: &str,
    r: &model::Resource,
    actor: &str,
    outcome: &str,
    detail: &str,
) -> CloudAuditEntry {
    let space = match r.sandbox.split_once(':') {
        Some((_, s)) => s.to_string(),
        None => r
            .tags
            .get(model::tag::SPACE)
            .cloned()
            .unwrap_or_else(|| r.sandbox.clone()),
    };
    CloudAuditEntry {
        action: action.into(),
        provider: r.provider.clone(),
        kind: r.resource_type.clone(),
        id: if r.id.is_empty() {
            r.name.clone()
        } else {
            r.id.clone()
        },
        space,
        actor: actor.into(),
        outcome: outcome.into(),
        detail: detail.into(),
        ..Default::default()
    }
}

/// Removes what looks like a secret from free text and caps its length.
/// Words that start like a known token or key are replaced, and the value
/// of `key=value` or `key: value` when the key names a secret.
pub fn redact(text: &str) -> String {
    const PREFIXES: &[&str] = &[
        "cmt_",
        "sk-",
        "sk_",
        "ya29.",
        "ghp_",
        "gho_",
        "github_pat_",
        "xoxb-",
        "xoxp-",
        "eyj",
        "key_",
    ];
    const SECRET_KEYS: &[&str] = &["token", "secret", "password", "passwd", "apikey", "api_key"];
    let mut out: Vec<String> = Vec::new();
    let mut hide_next = false;
    for word in text.split_whitespace() {
        let lower = word.to_ascii_lowercase();
        let bare = lower.trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if hide_next {
            hide_next = false;
            out.push("[redacted]".into());
            continue;
        }
        if lower
            .trim_start_matches(['"', '\'', '('])
            .starts_with("bearer")
        {
            hide_next = true;
            out.push(word.into());
            continue;
        }
        let stripped = lower.trim_start_matches(['"', '\'', '(', '=']);
        let aws_key = {
            let w = word.trim_matches(|c: char| !c.is_ascii_alphanumeric());
            (w.starts_with("AKIA") || w.starts_with("ASIA"))
                && w.len() >= 16
                && w.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        };
        if aws_key || PREFIXES.iter().any(|p| stripped.starts_with(p)) {
            out.push("[redacted]".into());
            continue;
        }
        if let Some((k, v)) = word.split_once(['=', ':']) {
            let k = k.to_ascii_lowercase();
            if SECRET_KEYS.iter().any(|s| k.contains(s)) {
                if v.is_empty() {
                    hide_next = true;
                    out.push(word.into());
                } else {
                    out.push(format!("{k}=[redacted]"));
                }
                continue;
            }
        }
        if SECRET_KEYS.contains(&bare) {
            hide_next = true;
        }
        out.push(word.into());
    }
    let mut s = out.join(" ");
    if s.chars().count() > DETAIL_MAX {
        s = s.chars().take(DETAIL_MAX).collect::<String>() + "...";
    }
    s
}

/// The log file of one cua home.
pub struct AuditLog {
    path: PathBuf,
    rotate_at: u64,
    lock: Mutex<()>,
}

impl AuditLog {
    /// The log under `home` (`<home>/cloud/audit.jsonl`).
    pub fn new(home: &Path) -> Self {
        AuditLog {
            path: home.join("cloud").join("audit.jsonl"),
            rotate_at: ROTATE_BYTES,
            lock: Mutex::new(()),
        }
    }

    /// Rotates at `bytes` instead of [`ROTATE_BYTES`] (tests).
    pub fn rotating_at(mut self, bytes: u64) -> Self {
        self.rotate_at = bytes;
        self
    }

    /// Where it lives.
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn rotated(&self) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(".1");
        PathBuf::from(p)
    }

    /// Appends `e` (its time set now when empty). Free text is redacted.
    pub fn append(&self, mut e: CloudAuditEntry) -> std::io::Result<()> {
        if e.ts.is_empty() {
            e.ts = model::rfc3339(model::now());
        }
        e.detail = redact(&e.detail);
        let mut line = serde_json::to_vec(&e).expect("an audit entry serializes");
        line.push(b'\n');
        let _g = self.lock.lock().expect("cloud audit lock");
        std::fs::create_dir_all(self.path.parent().expect("audit dir"))?;
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() > self.rotate_at) {
            std::fs::rename(&self.path, self.rotated())?;
        }
        let mut o = std::fs::OpenOptions::new();
        o.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            o.mode(0o600);
        }
        let mut f = o.open(&self.path)?;
        // One write call per line keeps lines whole when two processes
        // (the daemon and a CLI run) append at once.
        f.write_all(&line)
    }

    /// The newest `limit` entries, newest last (the rotated file first
    /// when the current one has fewer). Lines that do not parse are
    /// skipped.
    pub fn read(&self, limit: usize) -> std::io::Result<Vec<CloudAuditEntry>> {
        let _g = self.lock.lock().expect("cloud audit lock");
        let mut all: Vec<CloudAuditEntry> = Vec::new();
        for p in [self.rotated(), self.path.clone()] {
            match std::fs::read_to_string(&p) {
                Ok(t) => all.extend(t.lines().filter_map(|l| serde_json::from_str(l).ok())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        let skip = all.len().saturating_sub(limit);
        Ok(all.split_off(skip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_in_free_text_are_removed() {
        let r = redact(
            "failed: token=abc123 with Bearer eyJhbGci.x.y and cmt_deadbeef, key AKIAIOSFODNN7EXAMPLE password: hunter2",
        );
        for bad in [
            "abc123",
            "eyJhbGci",
            "cmt_deadbeef",
            "AKIAIOSFODNN7",
            "hunter2",
        ] {
            assert!(!r.contains(bad), "{bad} in {r}");
        }
        assert!(r.contains("failed:"));
        assert_eq!(redact("expired"), "expired");
        assert_eq!(redact("in asia-east1-a"), "in asia-east1-a");
        assert_eq!(redact("i-0abc quota exceeded"), "i-0abc quota exceeded");
        assert!(redact(&"x ".repeat(400)).len() < 400);
    }
}

#[cfg(test)]
mod log_tests {
    use super::*;

    fn line(n: usize) -> CloudAuditEntry {
        let mut e = entry("resource_created", "aws", "user");
        e.id = format!("i-{n}");
        e
    }

    #[test]
    fn lines_append_newest_last_and_the_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path());
        assert!(log.read(10).unwrap().is_empty(), "no file yet");
        for n in 0..5 {
            log.append(line(n)).unwrap();
        }
        let last3 = log.read(3).unwrap();
        assert_eq!(
            last3.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["i-2", "i-3", "i-4"]
        );
        assert!(last3[0].ts.ends_with('Z'), "{}", last3[0].ts);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(log.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A torn or foreign line is skipped, not fatal.
        std::fs::write(
            log.path(),
            format!(
                "{}\nnot json\n",
                std::fs::read_to_string(log.path()).unwrap().trim_end()
            ),
        )
        .unwrap();
        assert_eq!(log.read(100).unwrap().len(), 5);
    }

    #[test]
    fn it_rotates_past_the_limit_and_keeps_one_old_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path()).rotating_at(600);
        for n in 0..40 {
            log.append(line(n)).unwrap();
        }
        let old = dir.path().join("cloud").join("audit.jsonl.1");
        assert!(old.exists(), "rotated");
        assert!(!dir.path().join("cloud").join("audit.jsonl.2").exists());
        let size = |p: &Path| std::fs::metadata(p).unwrap().len();
        assert!(
            size(log.path()) <= 600 + 300,
            "current stays near the limit"
        );
        assert!(size(&old) > 0);
        // Reading spans both files, newest last, and the newest line is last.
        let all = log.read(1000).unwrap();
        assert_eq!(all.last().unwrap().id, "i-39");
        assert!(all.len() < 40, "the oldest rotated out: {}", all.len());
        let ids: Vec<usize> = all
            .iter()
            .map(|e| e.id.trim_start_matches("i-").parse().unwrap())
            .collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "in order: {ids:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(log.path()).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "a new file after rotation is private");
        }
    }

    #[test]
    fn the_default_limit_is_five_megabytes() {
        assert_eq!(ROTATE_BYTES, 5 * 1024 * 1024);
    }
}
