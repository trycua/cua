// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The exec ledger and the fencing epoch.
//!
//! `StartProcessRequest.request_id` makes a start idempotent. The ledger is an
//! append-only JSON Lines file in the data directory with one record per
//! state change of a key: `started` (written and synced before the process
//! is spawned), then `exited` with the end, output digest and a bounded
//! output tail. A key whose last record is `started` when the ledger is
//! loaded belonged to a previous server process, so its outcome is unknown:
//! it is recorded as `lost`. Loading compacts the file to the newest record
//! of the most recent [`MAX_KEYS`] keys.
//!
//! The ledger lives on the Space's own disk, so a checkpoint of the Space
//! captures it together with the files the commands changed: restoring a
//! checkpoint also restores the ledger to that point, and keys started after
//! it are unknown again, which matches the restored files.
//!
//! The fencing epoch is a monotonic counter in its own file. Nothing here
//! assigns epochs; clients (a durable harness) raise it, for example after
//! restoring the Space, so that requests from an older client fail.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use base64::Engine as _;
use cua_proto::env::v1::process_data::Output as DataOutput;
use cua_proto::env::v1::{ExecRecord, ExecState, ProcessData, ProcessEnd};
use serde::{Deserialize, Serialize};

/// Keys kept after compaction.
pub const MAX_KEYS: usize = 4096;
/// Bytes of output kept in an `exited` record.
pub const TAIL_BYTES: u64 = 64 * 1024;
/// Longest accepted `request_id`.
pub const MAX_REQUEST_ID: usize = 256;
/// Ledger file name in the data directory.
pub const LEDGER_FILE: &str = "exec-ledger.jsonl";
/// Epoch file name in the data directory.
pub const EPOCH_FILE: &str = "exec-epoch";
/// SHA-256 of no bytes.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// One stored chunk of output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailChunk {
    /// Combined output offset of the first byte.
    pub offset: u64,
    /// `o` stdout, `e` stderr, `p` PTY.
    pub kind: String,
    /// Base64 bytes.
    pub data: String,
}

fn is_zero(n: &i32) -> bool {
    *n == 0
}

/// How a process ended, as stored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredEnd {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub signal: i32,
    /// Raw terminating signal number.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub signal_number: i32,
    /// Conventional name of the terminating signal ("SEGV").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signal_name: String,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// Unix milliseconds.
    #[serde(default)]
    pub ended_at_ms: u64,
}

/// One line of the ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    /// `started`, `exited` or `lost`.
    pub state: String,
    #[serde(default)]
    pub epoch: u64,
    #[serde(default)]
    pub pid: u32,
    /// Unix milliseconds.
    #[serde(default)]
    pub started_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<StoredEnd>,
    #[serde(default)]
    pub output_bytes: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub output_sha256: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tail: Vec<TailChunk>,
}

impl Record {
    /// The proto form.
    pub fn to_proto(&self) -> ExecRecord {
        let state = match self.state.as_str() {
            "started" => ExecState::Running,
            "exited" => ExecState::Exited,
            "lost" => ExecState::Lost,
            _ => ExecState::Unspecified,
        };
        ExecRecord {
            request_id: self.id.clone(),
            state: state as i32,
            epoch: self.epoch,
            pid: self.pid,
            started_at: Some(crate::util::timestamp(from_ms(self.started_at_ms))),
            end: self
                .end
                .as_ref()
                .map(|e| end_to_proto(e, self.state == "lost")),
            output_bytes: self.output_bytes,
            output_sha256: self.output_sha256.clone(),
            tail: self.tail.iter().filter_map(tail_to_proto).collect(),
        }
    }
}

fn from_ms(ms: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(ms)
}

/// Unix milliseconds of `at`.
pub fn ms(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Stored form of a proto end.
pub fn end_from_proto(end: &ProcessEnd) -> StoredEnd {
    StoredEnd {
        exit_code: end.exit_code,
        signal: end.signal,
        signal_number: end.signal_number,
        signal_name: end.signal_name.clone(),
        timed_out: end.timed_out,
        error: end.error.clone(),
        ended_at_ms: end
            .ended_at
            .as_ref()
            .map(|t| (t.seconds.max(0) as u64) * 1000 + (t.nanos.max(0) as u64) / 1_000_000)
            .unwrap_or(0),
    }
}

/// Proto form of a stored end.
pub fn end_to_proto(end: &StoredEnd, lost: bool) -> ProcessEnd {
    ProcessEnd {
        exit_code: end.exit_code,
        signal: end.signal,
        signal_number: end.signal_number,
        signal_name: end.signal_name.clone(),
        timed_out: end.timed_out,
        error: end.error.clone(),
        ended_at: Some(crate::util::timestamp(from_ms(end.ended_at_ms))),
        lost,
    }
}

fn tail_to_proto(chunk: &TailChunk) -> Option<ProcessData> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(&chunk.data)
        .ok()?;
    let output = match chunk.kind.as_str() {
        "o" => DataOutput::Stdout(data),
        "e" => DataOutput::Stderr(data),
        "p" => DataOutput::Pty(data),
        _ => return None,
    };
    Some(ProcessData {
        offset: chunk.offset,
        output: Some(output),
    })
}

/// Stored form of one output chunk.
pub fn tail_chunk(offset: u64, kind: super::ring::OutKind, data: &[u8]) -> TailChunk {
    TailChunk {
        offset,
        kind: match kind {
            super::ring::OutKind::Stdout => "o",
            super::ring::OutKind::Stderr => "e",
            super::ring::OutKind::Pty => "p",
        }
        .into(),
        data: base64::engine::general_purpose::STANDARD.encode(data),
    }
}

/// The loaded ledger and epoch.
pub struct Ledger {
    dir: PathBuf,
    file: Option<File>,
    records: HashMap<String, Record>,
    order: VecDeque<String>,
    epoch: u64,
    cap: usize,
}

impl Ledger {
    /// Loads (and compacts) the ledger in `dir`, recording every key that was
    /// still running in a previous server process as lost.
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        Self::open_with_cap(dir, MAX_KEYS)
    }

    fn open_with_cap(dir: &Path, cap: usize) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(LEDGER_FILE);
        let mut records: HashMap<String, Record> = HashMap::new();
        let mut order: VecDeque<String> = VecDeque::new();
        if let Ok(file) = File::open(&path) {
            for line in BufReader::new(file).lines() {
                let Ok(line) = line else { break };
                // A torn last line (crash mid-append) is skipped.
                let Ok(record) = serde_json::from_str::<Record>(&line) else {
                    continue;
                };
                if !records.contains_key(&record.id) {
                    order.push_back(record.id.clone());
                }
                records.insert(record.id.clone(), record);
            }
        }
        let now = ms(SystemTime::now());
        for record in records.values_mut() {
            if record.state == "started" {
                record.state = "lost".into();
                record.end = Some(StoredEnd {
                    error:
                        "the command was running when cua-spacesd stopped; its outcome is unknown"
                            .into(),
                    ended_at_ms: now,
                    ..Default::default()
                });
            }
        }
        while order.len() > cap {
            if let Some(old) = order.pop_front() {
                records.remove(&old);
            }
        }
        // Compact: rewrite the newest record of each kept key.
        let tmp = dir.join(format!("{LEDGER_FILE}.tmp"));
        {
            let mut out = File::create(&tmp)?;
            for id in &order {
                if let Some(record) = records.get(id) {
                    serde_json::to_writer(&mut out, record)?;
                    out.write_all(b"\n")?;
                }
            }
            out.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        sync_dir(dir);
        let file = OpenOptions::new().append(true).open(&path)?;
        let epoch = fs::read_to_string(dir.join(EPOCH_FILE))
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0);
        Ok(Self {
            dir: dir.to_path_buf(),
            file: Some(file),
            records,
            order,
            epoch,
            cap,
        })
    }

    /// The current epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Raises the epoch to `epoch` (durably). Returns false when `epoch` is
    /// lower than the current one; equal is a no-op.
    pub fn advance(&mut self, epoch: u64) -> std::io::Result<bool> {
        if epoch < self.epoch {
            return Ok(false);
        }
        if epoch == self.epoch {
            return Ok(true);
        }
        let tmp = self.dir.join(format!("{EPOCH_FILE}.tmp"));
        {
            let mut out = File::create(&tmp)?;
            out.write_all(epoch.to_string().as_bytes())?;
            out.sync_all()?;
        }
        fs::rename(&tmp, self.dir.join(EPOCH_FILE))?;
        sync_dir(&self.dir);
        self.epoch = epoch;
        Ok(true)
    }

    /// The newest record of `id`.
    pub fn get(&self, id: &str) -> Option<&Record> {
        self.records.get(id)
    }

    /// Appends `record` durably (written and synced before returning).
    pub fn put(&mut self, record: Record) -> std::io::Result<()> {
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("ledger closed"))?;
        let mut line = serde_json::to_vec(&record)?;
        line.push(b'\n');
        file.write_all(&line)?;
        file.sync_data()?;
        if !self.records.contains_key(&record.id) {
            self.order.push_back(record.id.clone());
        }
        self.records.insert(record.id.clone(), record);
        // The file is compacted on the next load; memory stays bounded now.
        while self.order.len() > self.cap {
            if let Some(old) = self.order.pop_front() {
                self.records.remove(&old);
            }
        }
        Ok(())
    }
}

fn sync_dir(dir: &Path) {
    #[cfg(unix)]
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = dir;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(id: &str) -> Record {
        Record {
            id: id.into(),
            state: "started".into(),
            epoch: 3,
            pid: 42,
            started_at_ms: 1_000,
            end: None,
            output_bytes: 0,
            output_sha256: String::new(),
            tail: Vec::new(),
        }
    }

    #[test]
    fn reload_marks_unfinished_keys_lost_and_keeps_exits() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut ledger = Ledger::open(dir.path()).unwrap();
            ledger.put(started("a")).unwrap();
            ledger.put(started("b")).unwrap();
            let mut done = started("b");
            done.state = "exited".into();
            done.end = Some(StoredEnd {
                exit_code: Some(0),
                ..Default::default()
            });
            done.output_bytes = 2;
            done.tail = vec![tail_chunk(0, super::super::ring::OutKind::Stdout, b"hi")];
            ledger.put(done).unwrap();
        }
        let ledger = Ledger::open(dir.path()).unwrap();
        let a = ledger.get("a").unwrap().to_proto();
        assert_eq!(a.state, ExecState::Lost as i32);
        assert!(a.end.as_ref().unwrap().lost);
        let b = ledger.get("b").unwrap().to_proto();
        assert_eq!(b.state, ExecState::Exited as i32);
        assert_eq!(b.end.unwrap().exit_code, Some(0));
        assert_eq!(b.tail.len(), 1);
        // Compacted to one line per key.
        let text = fs::read_to_string(dir.path().join(LEDGER_FILE)).unwrap();
        assert_eq!(text.lines().count(), 2);
    }

    #[test]
    fn a_torn_last_line_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut ledger = Ledger::open(dir.path()).unwrap();
            ledger.put(started("a")).unwrap();
        }
        let mut file = OpenOptions::new()
            .append(true)
            .open(dir.path().join(LEDGER_FILE))
            .unwrap();
        file.write_all(b"{\"id\":\"b\",\"sta").unwrap();
        let ledger = Ledger::open(dir.path()).unwrap();
        assert!(ledger.get("b").is_none());
        assert_eq!(ledger.get("a").unwrap().state, "lost");
    }

    #[test]
    fn epoch_is_monotonic_and_durable() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut ledger = Ledger::open(dir.path()).unwrap();
            assert_eq!(ledger.epoch(), 0);
            assert!(ledger.advance(5).unwrap());
            assert!(ledger.advance(5).unwrap());
            assert!(!ledger.advance(4).unwrap());
        }
        assert_eq!(Ledger::open(dir.path()).unwrap().epoch(), 5);
    }

    #[test]
    fn keeps_the_most_recent_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = Ledger::open_with_cap(dir.path(), 8).unwrap();
        for i in 0..12 {
            let mut r = started(&format!("k{i}"));
            r.state = "exited".into();
            ledger.put(r).unwrap();
        }
        assert!(ledger.get("k3").is_none());
        assert!(ledger.get("k4").is_some());
        assert!(ledger.get("k11").is_some());
        drop(ledger);
        // The file still holds every line until the next load compacts it.
        let ledger = Ledger::open_with_cap(dir.path(), 8).unwrap();
        assert!(ledger.get("k3").is_none());
        assert!(ledger.get("k4").is_some());
        let text = fs::read_to_string(dir.path().join(LEDGER_FILE)).unwrap();
        assert_eq!(text.lines().count(), 8);
    }
}
