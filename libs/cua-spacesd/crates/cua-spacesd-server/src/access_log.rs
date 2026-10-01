// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The access log: who reached this machine, through what, and when.
//!
//! One JSON line per access, appended to `<data_dir>/access.log` (0600).
//! Each line carries the SHA-256 of the previous line's `hash` plus its own
//! body, so an edited, reordered or deleted line breaks the chain (the
//! reader in `cua-host` verifies it before showing anything). The log is
//! local and unauthenticated by design: it makes access visible and
//! tamper-evident, it is not a secret.
//!
//! Accesses are coalesced: one line per caller, credential kind and service
//! per [`COALESCE`] window. A caller that stays connected therefore writes a
//! line at least once per window, which is what lets the app say "connected
//! now" in direct mode (no relay presence there).
//!
//! Entries never contain credentials, only the caller's identity as the
//! server verified it (a relay-asserted account, a viewer grant) or the
//! credential kind ("token") with any client-claimed name marked as such.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// The first entry's `prev`.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// One line per (via, who, what) per this long.
pub const COALESCE: Duration = Duration::from_secs(60);
/// The file is rotated to `<name>.1` past this size (the chain continues).
pub const MAX_BYTES: u64 = 1024 * 1024;
/// Longest field kept (identities and service names are short).
const MAX_FIELD: usize = 200;

/// The hashed part of an entry. Field order is the canonical form; the
/// reader in `cua-host` serializes the same struct to verify.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessBody {
    /// 1-based sequence number (continues across rotation).
    pub seq: u64,
    /// Unix milliseconds.
    pub ts_ms: u64,
    /// How the caller authenticated: `relay`, `viewer` or `token`.
    pub via: String,
    /// Who: the verified identity, or `token` (plus a claimed name).
    pub who: String,
    /// What: the gRPC service (`ProcessService`, ...) or `MCP`.
    pub what: String,
    /// The previous entry's `hash` (or [`GENESIS`]).
    pub prev: String,
}

/// One stored line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessEntry {
    /// The hashed body.
    #[serde(flatten)]
    pub body: AccessBody,
    /// `sha256(prev "\n" body_json)`, hex.
    pub hash: String,
}

/// The chain hash of `body`.
pub fn entry_hash(body: &AccessBody) -> String {
    let json = serde_json::to_string(body).expect("serializable");
    let mut h = Sha256::new();
    h.update(body.prev.as_bytes());
    h.update(b"\n");
    h.update(json.as_bytes());
    hex::encode(h.finalize())
}

fn clip(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    if s.len() <= MAX_FIELD {
        return s;
    }
    let mut end = MAX_FIELD;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

struct State {
    seq: u64,
    prev: String,
    last: HashMap<(String, String, String), Instant>,
}

/// An append-only, hash-chained access log.
pub struct AccessLog {
    path: PathBuf,
    state: Mutex<State>,
}

impl std::fmt::Debug for AccessLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessLog")
            .field("path", &self.path)
            .finish()
    }
}

/// The last entry of `path`, if it has a readable one.
fn last_entry(path: &Path) -> Option<AccessEntry> {
    let f = std::fs::File::open(path).ok()?;
    BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .last()
        .and_then(|l| serde_json::from_str(&l).ok())
}

impl AccessLog {
    /// Opens (or starts) the log at `path`, continuing the chain from its
    /// last entry (or the rotated file's).
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let last = last_entry(&path).or_else(|| last_entry(&rotated(&path)));
        let (seq, prev) = match last {
            Some(e) => (e.body.seq, e.hash),
            None => (0, GENESIS.to_string()),
        };
        Self {
            path,
            state: Mutex::new(State {
                seq,
                prev,
                last: HashMap::new(),
            }),
        }
    }

    /// Where it writes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Records an access, unless the same (via, who, what) was recorded less
    /// than [`COALESCE`] ago. Returns whether a line was written. Failures
    /// are logged, never fatal to the request.
    pub fn record(&self, via: &str, who: &str, what: &str) -> bool {
        self.record_at(via, who, what, Instant::now(), SystemTime::now())
    }

    fn record_at(&self, via: &str, who: &str, what: &str, now: Instant, wall: SystemTime) -> bool {
        let (via, who, what) = (clip(via), clip(who), clip(what));
        let mut st = self.state.lock().expect("access log lock");
        let key = (via.clone(), who.clone(), what.clone());
        if st
            .last
            .get(&key)
            .is_some_and(|t| now.saturating_duration_since(*t) < COALESCE)
        {
            return false;
        }
        // Bound the coalescing table (distinct callers are few).
        if st.last.len() > 4096 {
            st.last
                .retain(|_, t| now.saturating_duration_since(*t) < COALESCE);
        }
        let body = AccessBody {
            seq: st.seq + 1,
            ts_ms: wall
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            via,
            who,
            what,
            prev: st.prev.clone(),
        };
        let hash = entry_hash(&body);
        let entry = AccessEntry { body, hash };
        match self.append(&entry) {
            Ok(()) => {
                st.seq = entry.body.seq;
                st.prev = entry.hash;
                st.last.insert(key, now);
                true
            }
            Err(e) => {
                tracing::warn!(path = %self.path.display(), error = %e, "access log write failed");
                false
            }
        }
    }

    fn append(&self, entry: &AccessEntry) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() >= MAX_BYTES) {
            std::fs::rename(&self.path, rotated(&self.path))?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut f = opts.open(&self.path)?;
        let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
        line.push('\n');
        f.write_all(line.as_bytes())
    }
}

/// `<path>.1`.
pub fn rotated(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".1");
    path.with_file_name(name)
}

/// Verifies the chain of the entries in `path` (after the rotated file's,
/// when present). Returns how many entries were checked, or where it broke.
pub fn verify(path: &Path) -> Result<u64, String> {
    let mut prev: Option<String> = None;
    let mut n = 0;
    for p in [rotated(path), path.to_path_buf()] {
        let Ok(f) = std::fs::File::open(&p) else {
            continue;
        };
        for (i, line) in BufReader::new(f).lines().enumerate() {
            let line = line.map_err(|e| format!("{}: {e}", p.display()))?;
            if line.trim().is_empty() {
                continue;
            }
            let e: AccessEntry = serde_json::from_str(&line)
                .map_err(|e| format!("{} line {}: {e}", p.display(), i + 1))?;
            if prev.as_ref().is_some_and(|prev| &e.body.prev != prev) {
                return Err(format!("{} line {}: chain broken", p.display(), i + 1));
            }
            if entry_hash(&e.body) != e.hash {
                return Err(format!("{} line {}: hash mismatch", p.display(), i + 1));
            }
            prev = Some(e.hash);
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_coalesce_and_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let log = AccessLog::open(&path);
        let t0 = Instant::now();
        let w = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert!(log.record_at("relay", "ada@example.test", "ProcessService", t0, w));
        // Same caller and service within the window: coalesced.
        assert!(!log.record_at(
            "relay",
            "ada@example.test",
            "ProcessService",
            t0 + Duration::from_secs(5),
            w
        ));
        // Another service is its own line.
        assert!(log.record_at("relay", "ada@example.test", "FilesystemService", t0, w));
        // After the window, the same access is written again.
        assert!(log.record_at(
            "relay",
            "ada@example.test",
            "ProcessService",
            t0 + COALESCE,
            w
        ));
        assert_eq!(verify(&path).unwrap(), 3);

        // A reopened log continues the chain.
        let again = AccessLog::open(&path);
        assert!(again.record("token", "token", "MCP"));
        assert_eq!(verify(&path).unwrap(), 4);
    }

    #[test]
    fn tampering_breaks_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let log = AccessLog::open(&path);
        log.record("relay", "ada@example.test", "ProcessService");
        log.record("viewer", "viewer:bob", "DesktopService");
        log.record("token", "token", "MCP");
        let text = std::fs::read_to_string(&path).unwrap();
        // Rewriting who accessed breaks the hash.
        std::fs::write(&path, text.replace("viewer:bob", "viewer:eve")).unwrap();
        assert!(verify(&path).unwrap_err().contains("hash mismatch"));
        // Deleting a line breaks the chain.
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(verify(&path).unwrap_err().contains("chain broken"));
    }

    #[test]
    fn rotation_keeps_the_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let log = AccessLog::open(&path);
        let who = "x".repeat(150);
        let mut i = 0;
        while !rotated(&path).exists() {
            log.record("token", &who, &format!("Service{i}"));
            i += 1;
            assert!(i < 100_000, "never rotated");
        }
        log.record("token", &who, "after");
        let n = verify(&path).unwrap();
        assert_eq!(n, i + 1);
    }

    #[test]
    fn fields_are_clipped_and_single_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let log = AccessLog::open(&path);
        log.record("token", &format!("a\nb{}", "é".repeat(300)), "MCP");
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 1);
        let e: AccessEntry = serde_json::from_str(text.trim()).unwrap();
        assert!(e.body.who.len() <= MAX_FIELD && e.body.who.starts_with("ab"));
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_and_no_symlink_follow() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        AccessLog::open(&path).record("token", "token", "MCP");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let other = dir.path().join("elsewhere");
        std::fs::write(&other, "").unwrap();
        let link = dir.path().join("linked.log");
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(!AccessLog::open(&link).record("token", "token", "MCP"));
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "");
    }

    /// The canonical form `cua-host` verifies (keep in sync with its copy).
    #[test]
    fn canonical_hash_fixture() {
        let body = AccessBody {
            seq: 1,
            ts_ms: 1_700_000_000_000,
            via: "relay".into(),
            who: "ada@example.test".into(),
            what: "ProcessService".into(),
            prev: GENESIS.into(),
        };
        assert_eq!(entry_hash(&body), FIXTURE_HASH);
    }

    const FIXTURE_HASH: &str = "3fa4bad8455fa8b9fd268f97865102bb7c638b1b8a6f8a20c75d8742e9793659";
}
