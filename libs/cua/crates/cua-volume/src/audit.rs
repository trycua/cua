// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The drive's audit log: one hash-chained JSON line per decision.
//!
//! Every grant, revocation, access request, refused access, blocked secret,
//! sync and lease change is a line; reads and writes by agents are lines
//! too. Each line's hash covers the previous hash, so an edited or truncated
//! log is reported by [`AuditLog::verify`] rather than shown as clean.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Error, Result, now_ms, sha256_hex};

/// The chain's first `prev`.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";
/// Lines read at most.
const MAX_LINES: usize = 500_000;

/// The hashed part of a line (field order is the canonical form).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub ts_ms: u64,
    /// `user`, `agent:ada`, `space:local-work`.
    pub principal: String,
    /// `read`, `write`, `delete`, `grant`, `revoke`, `request`, `approve`,
    /// `deny`, `denied`, `secret_blocked`, `sync_in`, `sync_out`, `lease`.
    pub action: String,
    /// The key or prefix.
    pub path: String,
    /// Free text (never file contents or secret values).
    pub detail: String,
    pub prev: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Line {
    #[serde(flatten)]
    body: Event,
    hash: String,
}

fn hash(body: &Event) -> String {
    let json = serde_json::to_string(body).expect("serializable");
    sha256_hex(format!("{}\n{json}", body.prev).as_bytes())
}

/// An append-only, hash-chained log file.
#[derive(Clone, Debug)]
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    pub fn new(path: impl Into<PathBuf>) -> AuditLog {
        AuditLog { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn last(&self) -> Result<(u64, String)> {
        let f = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, GENESIS.into())),
            Err(e) => return Err(e.into()),
        };
        let mut last = (0, GENESIS.to_string());
        for line in BufReader::new(f).lines().take(MAX_LINES) {
            let line = line?;
            if let Ok(l) = serde_json::from_str::<Line>(&line) {
                last = (l.body.seq, l.hash);
            }
        }
        Ok(last)
    }

    /// Appends one event (under an exclusive lock across processes).
    pub fn append(&self, principal: &str, action: &str, path: &str, detail: &str) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        cua_home::guard_write(&self.path)?;
        let lock_path = self.path.with_extension("lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)?;
        lock.lock()?;
        // The head sidecar makes an append O(1); it is trusted only while the
        // log's length is the one it recorded (anything else rescans).
        let head_path = self.path.with_extension("head");
        let len = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let cached = fs::read(&head_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<(u64, String, u64)>(&b).ok())
            .filter(|(_, _, l)| *l == len);
        let (seq, prev) = match cached {
            Some((seq, hash, _)) => (seq, hash),
            None => self.last()?,
        };
        let body = Event {
            seq: seq + 1,
            ts_ms: now_ms(),
            principal: principal.into(),
            action: action.into(),
            path: path.into(),
            detail: detail.chars().take(500).collect(),
            prev,
        };
        let line = Line {
            hash: hash(&body),
            body,
        };
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(f, "{}", serde_json::to_string(&line)?)?;
        f.sync_data()?;
        let len = f.metadata()?.len();
        let _ = fs::write(
            &head_path,
            serde_json::to_vec(&(line.body.seq, &line.hash, len))?,
        );
        Ok(())
    }

    /// The newest `limit` events, newest first, and whether the whole chain
    /// verified (`Err` names the first bad line).
    pub fn tail(&self, limit: usize) -> Result<(Vec<Event>, std::result::Result<(), String>)> {
        let f = match File::open(&self.path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], Ok(()))),
            Err(e) => return Err(e.into()),
        };
        let mut all = std::collections::VecDeque::new();
        let mut prev = GENESIS.to_string();
        let mut verdict = Ok(());
        for (i, line) in BufReader::new(f).lines().enumerate() {
            if i >= MAX_LINES {
                verdict = Err("audit log too long to verify".into());
                break;
            }
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let Ok(l) = serde_json::from_str::<Line>(&line) else {
                verdict = Err(format!("line {}: malformed", i + 1));
                break;
            };
            if l.body.prev != prev {
                verdict = Err(format!("line {}: chain broken", i + 1));
                break;
            }
            if hash(&l.body) != l.hash {
                verdict = Err(format!("line {}: altered", i + 1));
                break;
            }
            prev = l.hash;
            all.push_back(l.body);
            if all.len() > limit.max(1) {
                all.pop_front();
            }
        }
        Ok((all.into_iter().rev().collect(), verdict))
    }

    /// Verifies the chain.
    pub fn verify(&self) -> Result<()> {
        self.tail(1)?.1.map_err(Error::Backend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chains_and_detects_edits() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join("audit.jsonl"));
        assert!(log.tail(5).unwrap().0.is_empty());
        log.append("user", "grant", "agents/bob/", "agent:ada r")
            .unwrap();
        log.append("agent:ada", "read", "agents/bob/x", "").unwrap();
        log.append("agent:ada", "denied", "agents/carol/x", "read")
            .unwrap();
        let (tail, ok) = log.tail(2).unwrap();
        assert!(ok.is_ok());
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].action, "denied");
        assert_eq!(tail[0].seq, 3);
        let text = std::fs::read_to_string(log.path()).unwrap();
        std::fs::write(log.path(), text.replace("carol", "bob")).unwrap();
        assert!(log.verify().unwrap_err().to_string().contains("altered"));
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(log.path(), format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(
            log.verify()
                .unwrap_err()
                .to_string()
                .contains("chain broken")
        );
    }
}
