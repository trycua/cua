//! Reads the host's access log: who reached this machine, through what, and
//! when.
//!
//! `cua-spacesd` appends one hash-chained JSON line per access to
//! `<data-dir>/access.log` (its `access_log` module). This reader verifies
//! the chain before trusting anything in it: a log that was edited or had
//! lines removed is reported as such rather than shown as clean.

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// Accesses shown by default.
pub const RECENT_LIMIT: usize = 20;
/// A caller with an access this recent counts as connected now. The driver
/// writes at most one line per caller and service per minute while the
/// caller keeps using it, so two minutes covers an active session.
pub const ACTIVE_WINDOW_MS: u64 = 120_000;
/// Lines read at most (the driver rotates at 1 MiB, so this is generous).
const MAX_LINES: usize = 200_000;

/// The hashed part of a line (field order is the canonical form; keep in
/// sync with `cua-spacesd-server`'s `AccessBody`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Body {
    seq: u64,
    ts_ms: u64,
    via: String,
    who: String,
    what: String,
    prev: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Line {
    #[serde(flatten)]
    body: Body,
    hash: String,
}

fn entry_hash(body: &Body) -> String {
    let json = serde_json::to_string(body).expect("serializable");
    let mut h = Sha256::new();
    h.update(body.prev.as_bytes());
    h.update(b"\n");
    h.update(json.as_bytes());
    hex::encode(h.finalize())
}

/// One access, for display.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessRecord {
    /// Unix milliseconds.
    pub at_ms: u64,
    /// How the caller authenticated: `relay`, `viewer` or `token`.
    pub via: String,
    /// Who (a relay-verified account, a viewer, or the token holder).
    pub who: String,
    /// What they used (`ProcessService`, `DesktopService`, `MCP`, ...).
    pub what: String,
}

/// The log's recent entries and whether its chain verified.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccessReport {
    /// Newest first, at most the requested limit.
    pub recent: Vec<AccessRecord>,
    /// Set when the chain does not verify (edited or truncated log).
    pub error: Option<String>,
}

fn rotated(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".1");
    path.with_file_name(name)
}

/// Reads and verifies `path` (after its rotated `<path>.1`), keeping the
/// newest `limit` accesses. A missing log is an empty report.
pub fn read(path: &Path, limit: usize) -> AccessReport {
    let mut report = AccessReport::default();
    let mut all: std::collections::VecDeque<AccessRecord> = Default::default();
    let mut prev: Option<String> = None;
    let mut n = 0usize;
    'files: for p in [rotated(path), path.to_path_buf()] {
        let Ok(f) = std::fs::File::open(&p) else {
            continue;
        };
        for (i, line) in BufReader::new(f).lines().enumerate() {
            n += 1;
            if n > MAX_LINES {
                report.error = Some("access log too long to verify".into());
                break 'files;
            }
            let Ok(line) = line else {
                report.error = Some(format!("{} line {}: unreadable", p.display(), i + 1));
                break 'files;
            };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(e) = serde_json::from_str::<Line>(&line) else {
                report.error = Some(format!("{} line {}: malformed", p.display(), i + 1));
                break 'files;
            };
            if prev.as_ref().is_some_and(|prev| &e.body.prev != prev) {
                report.error = Some(format!("{} line {}: chain broken", p.display(), i + 1));
                break 'files;
            }
            if entry_hash(&e.body) != e.hash {
                report.error = Some(format!("{} line {}: altered", p.display(), i + 1));
                break 'files;
            }
            prev = Some(e.hash);
            all.push_back(AccessRecord {
                at_ms: e.body.ts_ms,
                via: e.body.via,
                who: e.body.who,
                what: e.body.what,
            });
            if all.len() > limit.max(1) * 8 {
                all.pop_front();
            }
        }
    }
    report.recent = all.into_iter().rev().take(limit).collect();
    report
}

/// The distinct callers with an access in the last [`ACTIVE_WINDOW_MS`]
/// before `now_ms` (newest first).
pub fn active(recent: &[AccessRecord], now_ms: u64) -> Vec<&AccessRecord> {
    let mut seen = std::collections::HashSet::new();
    recent
        .iter()
        .filter(|r| now_ms.saturating_sub(r.at_ms) <= ACTIVE_WINDOW_MS)
        .filter(|r| seen.insert((r.via.as_str(), r.who.as_str())))
        .collect()
}

#[cfg(test)]
const TEST_GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Writes a valid chained log of `(ts_ms, via, who, what)` rows (tests).
#[cfg(test)]
pub(crate) fn write_log(path: &Path, rows: &[(u64, &str, &str, &str)]) {
    let mut prev = TEST_GENESIS.to_string();
    use std::io::Write as _;
    let mut f = std::fs::File::create(path).unwrap();
    for (i, (ts, via, who, what)) in rows.iter().enumerate() {
        let body = Body {
            seq: i as u64 + 1,
            ts_ms: *ts,
            via: (*via).into(),
            who: (*who).into(),
            what: (*what).into(),
            prev: prev.clone(),
        };
        let hash = entry_hash(&body);
        let mut v = serde_json::to_value(&body).unwrap();
        v["hash"] = hash.clone().into();
        writeln!(f, "{v}").unwrap();
        prev = hash;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    /// The same fixture `cua-spacesd-server`'s writer pins.
    #[test]
    fn canonical_hash_matches_the_driver() {
        let body = Body {
            seq: 1,
            ts_ms: 1_700_000_000_000,
            via: "relay".into(),
            who: "ada@example.test".into(),
            what: "ProcessService".into(),
            prev: GENESIS.into(),
        };
        assert_eq!(
            entry_hash(&body),
            "3fa4bad8455fa8b9fd268f97865102bb7c638b1b8a6f8a20c75d8742e9793659"
        );
    }

    #[test]
    fn reads_newest_first_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        assert_eq!(read(&path, 5), AccessReport::default(), "no log yet");
        write_log(
            &path,
            &[
                (1_000, "token", "token", "SystemService"),
                (2_000, "relay", "Ada (acct-1)", "ProcessService"),
                (3_000, "viewer", "Bob (viewer:bob)", "DesktopService"),
            ],
        );
        let r = read(&path, 2);
        assert_eq!(r.error, None);
        assert_eq!(r.recent.len(), 2);
        assert_eq!(r.recent[0].who, "Bob (viewer:bob)");
        assert_eq!(r.recent[1].what, "ProcessService");
    }

    #[test]
    fn edits_and_deletions_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        write_log(
            &path,
            &[
                (1_000, "token", "token", "SystemService"),
                (2_000, "relay", "Ada (acct-1)", "ProcessService"),
                (3_000, "relay", "Eve (acct-2)", "FilesystemService"),
            ],
        );
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("Eve (acct-2)", "Ada (acct-1)")).unwrap();
        assert!(read(&path, 5).error.unwrap().contains("altered"));
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(read(&path, 5).error.unwrap().contains("chain broken"));
    }

    #[test]
    fn active_is_recent_and_distinct() {
        let rows = [
            AccessRecord {
                at_ms: 1_000_000,
                via: "token".into(),
                who: "token".into(),
                what: "MCP".into(),
            },
            AccessRecord {
                at_ms: 990_000,
                via: "token".into(),
                who: "token".into(),
                what: "ProcessService".into(),
            },
            AccessRecord {
                at_ms: 1_000_000 - ACTIVE_WINDOW_MS - 1,
                via: "relay".into(),
                who: "Ada".into(),
                what: "MCP".into(),
            },
        ];
        let now = active(&rows, 1_000_000);
        assert_eq!(now.len(), 1);
        assert_eq!(now[0].who, "token");
        assert!(active(&rows, 1_000_000 + ACTIVE_WINDOW_MS + 1).is_empty());
    }
}
