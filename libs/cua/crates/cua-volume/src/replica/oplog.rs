// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The replication log: what the owner did, in order.
//!
//! One JSON object per line in `oplog.jsonl`. Every machine that holds a
//! copy keeps the records it applied (the owner the ones it made), so any
//! of them can become the owner and replay history to the others. A record
//! carries the previous head of its key (`prev`), so a machine that finds
//! itself on the wrong side of a takeover can move its heads back without
//! losing a version.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::era::Pos;
use crate::backend::ObjectMeta;
use crate::{Error, Result};

/// What happened to one key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// `meta` became the head of its key.
    Put {
        meta: ObjectMeta,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prev: Option<ObjectMeta>,
    },
    /// `key` was deleted (a marker with `version`).
    Delete {
        key: String,
        version: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prev: Option<ObjectMeta>,
    },
}

impl Op {
    pub fn key(&self) -> &str {
        match self {
            Op::Put { meta, .. } => &meta.key,
            Op::Delete { key, .. } => key,
        }
    }

    pub fn prev(&self) -> Option<&ObjectMeta> {
        match self {
            Op::Put { prev, .. } | Op::Delete { prev, .. } => prev.as_ref(),
        }
    }
}

/// One log entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub pos: Pos,
    pub op: Op,
    /// Wall time at the owner (display only).
    #[serde(default)]
    pub at_ms: u64,
}

#[derive(Serialize, Deserialize)]
struct Header {
    floor: Pos,
}

/// The log on disk plus the records kept in memory.
#[derive(Debug)]
pub struct OpLog {
    path: PathBuf,
    recs: Vec<Record>,
    keep: usize,
    /// Everything at or before this position is gone (trimmed, or the
    /// machine joined by snapshot): a replica behind it needs a snapshot.
    floor: Pos,
}

impl OpLog {
    /// Opens (creating) `dir/oplog.jsonl`. A torn last line (a crash while
    /// appending) is dropped.
    pub fn open(dir: &Path, keep: usize) -> Result<OpLog> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("oplog.jsonl");
        let mut recs = vec![];
        let mut floor = Pos::ZERO;
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                if let Ok(h) = serde_json::from_str::<Header>(line) {
                    floor = h.floor;
                    continue;
                }
                match serde_json::from_str::<Record>(line) {
                    Ok(r) => recs.push(r),
                    Err(_) => break,
                }
            }
        }
        let log = OpLog {
            path,
            recs,
            keep,
            floor,
        };
        log.rewrite()?;
        Ok(log)
    }

    fn rewrite(&self) -> Result<()> {
        let mut out = Vec::new();
        serde_json::to_writer(&mut out, &Header { floor: self.floor })?;
        out.push(b'\n');
        for r in &self.recs {
            serde_json::to_writer(&mut out, r)?;
            out.push(b'\n');
        }
        cua_home::guard_write(&self.path)?;
        let tmp = self.path.with_extension("tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&out)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// The newest record's position (zero when empty).
    pub fn tail(&self) -> Pos {
        self.recs.last().map_or(self.floor, |r| r.pos)
    }

    /// Starts an empty log at `pos` (a machine that joined by snapshot has
    /// no records before it).
    pub fn reset_to(&mut self, pos: Pos) -> Result<()> {
        self.recs.clear();
        self.floor = pos;
        self.rewrite()
    }

    /// Wall time of the newest record (what it recorded as `at_ms`).
    pub fn newest_at_ms(&self) -> Option<u64> {
        self.recs.iter().map(|r| r.at_ms).max()
    }

    /// The oldest record kept.
    pub fn head(&self) -> Option<Pos> {
        self.recs.first().map(|r| r.pos)
    }

    pub fn len(&self) -> usize {
        self.recs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.recs.is_empty()
    }

    /// Appends `rec` (its position must be after the tail) and makes it
    /// durable.
    pub fn append(&mut self, rec: Record) -> Result<()> {
        if rec.pos <= self.tail() {
            return Err(Error::Backend(format!(
                "oplog: {} is not after {}",
                rec.pos,
                self.tail()
            )));
        }
        let mut line = serde_json::to_vec(&rec)?;
        line.push(b'\n');
        cua_home::guard_write(&self.path)?;
        let mut f = std::fs::OpenOptions::new().append(true).open(&self.path)?;
        f.write_all(&line)?;
        f.sync_data()?;
        self.recs.push(rec);
        if self.recs.len() > self.keep.saturating_mul(2) {
            let drop = self.recs.len() - self.keep;
            if let Some(last) = self.recs[drop - 1..drop].first() {
                self.floor = last.pos;
            }
            self.recs.drain(..drop);
            self.rewrite()?;
        }
        Ok(())
    }

    /// Appends many records with one disk barrier (a batch of writes).
    pub fn append_batch(&mut self, recs: Vec<Record>) -> Result<()> {
        if recs.is_empty() {
            return Ok(());
        }
        let mut prev = self.tail();
        let mut bytes = Vec::new();
        for r in &recs {
            if r.pos <= prev {
                return Err(Error::Backend(format!(
                    "oplog: {} is not after {prev}",
                    r.pos
                )));
            }
            prev = r.pos;
            serde_json::to_writer(&mut bytes, r)?;
            bytes.push(b'\n');
        }
        cua_home::guard_write(&self.path)?;
        let mut f = std::fs::OpenOptions::new().append(true).open(&self.path)?;
        f.write_all(&bytes)?;
        f.sync_data()?;
        self.recs.extend(recs);
        if self.recs.len() > self.keep.saturating_mul(2) {
            let drop = self.recs.len() - self.keep;
            if let Some(last) = self.recs[drop - 1..drop].first() {
                self.floor = last.pos;
            }
            self.recs.drain(..drop);
            self.rewrite()?;
        }
        Ok(())
    }

    /// Records after `pos`, at most `max`. `None` when the log no longer
    /// reaches back to `pos` (the caller sends a snapshot instead).
    pub fn after(&self, pos: Pos, max: usize) -> Option<Vec<Record>> {
        if pos < self.floor {
            return None;
        }
        let i = self.recs.partition_point(|r| r.pos <= pos);
        Some(self.recs[i..].iter().take(max).cloned().collect())
    }

    /// Records in `(after, up_to]`.
    pub fn range(&self, after: Pos, up_to: Pos) -> Vec<Record> {
        self.recs
            .iter()
            .filter(|r| r.pos > after && r.pos <= up_to)
            .cloned()
            .collect()
    }

    /// Drops every record after `keep` and returns them (newest first):
    /// the divergent tail.
    pub fn cut_after(&mut self, keep: Pos) -> Result<Vec<Record>> {
        let i = self.recs.partition_point(|r| r.pos <= keep);
        let mut tail: Vec<Record> = self.recs.drain(i..).collect();
        tail.reverse();
        if !tail.is_empty() {
            self.rewrite()?;
        }
        Ok(tail)
    }

    /// Whether the record at `pos` is held.
    pub fn has(&self, pos: Pos) -> bool {
        self.recs.binary_search_by(|r| r.pos.cmp(&pos)).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(epoch: u64, seq: u64, key: &str) -> Record {
        Record {
            pos: Pos { epoch, seq },
            op: Op::Put {
                meta: ObjectMeta {
                    key: key.into(),
                    size: 1,
                    etag: "e".into(),
                    version: format!("v{epoch}.{seq}"),
                    modified_ms: 0,
                },
                prev: None,
            },
            at_ms: 0,
        }
    }

    #[test]
    fn append_replay_cut_and_reopen() {
        let d = tempfile::tempdir().unwrap();
        let mut l = OpLog::open(d.path(), 100).unwrap();
        for s in 1..=5 {
            l.append(put(1, s, &format!("k{s}"))).unwrap();
        }
        assert!(l.append(put(1, 5, "dup")).is_err());
        assert_eq!(l.tail(), Pos { epoch: 1, seq: 5 });
        assert_eq!(l.after(Pos { epoch: 1, seq: 3 }, 10).unwrap().len(), 2);
        assert_eq!(l.after(Pos::ZERO, 10).unwrap().len(), 5);
        let tail = l.cut_after(Pos { epoch: 1, seq: 3 }).unwrap();
        assert_eq!(tail.len(), 2);
        assert_eq!(tail[0].pos.seq, 5, "newest first");
        l.append(put(2, 1, "later")).unwrap();
        let again = OpLog::open(d.path(), 100).unwrap();
        assert_eq!(again.tail(), Pos { epoch: 2, seq: 1 });
        assert_eq!(again.len(), 4);
    }

    #[test]
    fn a_torn_last_line_is_dropped_and_trimming_asks_for_a_snapshot() {
        let d = tempfile::tempdir().unwrap();
        {
            let mut l = OpLog::open(d.path(), 4).unwrap();
            for s in 1..=3 {
                l.append(put(1, s, "k")).unwrap();
            }
        }
        let p = d.path().join("oplog.jsonl");
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"pos\":{\"epoch\":1,\"se").unwrap();
        let mut l = OpLog::open(d.path(), 4).unwrap();
        assert_eq!(l.len(), 3);
        for s in 4..=20 {
            l.append(put(1, s, "k")).unwrap();
        }
        // The log kept only the newest; a new replica needs a snapshot.
        assert!(l.after(Pos::ZERO, 10).is_none());
        let head = l.head().unwrap();
        assert!(l.after(head, 100).is_some());
        assert!(l.after(Pos { epoch: 1, seq: 1 }, 100).is_none());
    }
}
