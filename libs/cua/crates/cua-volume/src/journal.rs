// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Durable session journals on the Volume.
//!
//! A durable agent run keeps its journal (`events.jsonl`, `tools.jsonl`,
//! `state.json`) in a local state directory behind a local epoch fence. The
//! host that drives the run mirrors it here, so a re-drive on another host
//! (or in a re-placed Space) starts from the same journal:
//!
//! ```text
//! spaces/<space>/sessions/<session>/fence                    the epoch floor (IfMatch CAS)
//! spaces/<space>/sessions/<session>/<epoch 6>/<chunk 8>.jsonl  immutable chunks (create-only)
//! ```
//!
//! A chunk is JSON lines: a header `{"h":1,"epoch","index","end":{file:
//! offset},"ts_ms"}` (the byte offsets of each journal file *after* this
//! chunk), then records `{"f":file,"off":offset,"l":line}` (one journal line
//! without its newline, at its byte offset in the local file) and at most one
//! `{"f":"state.json","l":body}` snapshot. Chunks are never rewritten:
//! created with `IfNoneMatch`, so two shippers racing for the same chunk
//! cannot both land, and nothing written can be replaced (the Volume's
//! whole-object last-writer-wins model never applies).
//!
//! Secrets: every line goes through the scanner and anything that looks
//! like a key is blanked in place (same length, so offsets stay right)
//! before it reaches the Volume.
//!
//! # The fence
//!
//! `fence` holds `{"epoch","holder","ts_ms"}`, raised only by
//! compare-and-swap (`IfNoneMatch` when absent, `IfMatch(etag)` after).
//! A shipper of a lower epoch refuses to ship. On the S3 backend the CAS is
//! atomic across machines; on the `fs` backend it is atomic only between
//! processes of one machine (the store lock), so two hosts sharing an `fs`
//! Volume through a network mount are not fenced against each other.
//!
//! # Restore
//!
//! [`restore`] rebuilds the journal from the chunks, epochs in order. A
//! later epoch supersedes what an earlier one shipped from the later one's
//! first offset on (a stale driver that kept shipping after the next epoch
//! started), and each file is rebuilt contiguously from offset 0.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::backend::{Backend, Condition, ObjectMeta};
use crate::{Error, Result, now_ms, path};

/// The sessions folder inside a Space's folder.
pub const SESSIONS: &str = "sessions/";
/// The fence object's name inside a session's folder.
pub const FENCE: &str = "fence";
/// The append-only journal files that are shipped line by line, in the
/// order a chunk carries them (the tool ledger first: intents).
pub const LINE_FILES: [&str; 2] = ["tools.jsonl", "events.jsonl"];
/// The state snapshot (the newest one wins on restore).
pub const STATE_FILE: &str = "state.json";
/// The local epoch floor file of a state directory.
pub const EPOCH_FILE: &str = "epoch";
/// Most bytes of journal lines one chunk carries.
pub const MAX_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// A session name: 1 to 64 of `[A-Za-z0-9._-]`, starting with a letter or
/// digit (run ids `run-1a2b3c4d`, coordinator session names, member names).
pub fn valid_session_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// The session's folder: `spaces/<space folder>/sessions/<session>/`.
/// `space` is a Space id (`local:dev`) or a stable name (a durable Space's
/// name), mapped with [`path::space_folder_name`].
pub fn session_prefix(space: &str, session: &str) -> Result<String> {
    if !valid_session_name(session) {
        return Err(Error::Invalid(format!(
            "session name {session:?}: use 1-64 of A-Z, a-z, 0-9, `.`, `_`, `-`"
        )));
    }
    Ok(format!("{}{SESSIONS}{session}/", path::space_folder(space)))
}

/// The key of chunk `index` of `epoch`.
pub fn chunk_key(prefix: &str, epoch: u64, index: u64) -> String {
    format!("{prefix}{epoch:06}/{index:08}.jsonl")
}

fn parse_chunk_key(prefix: &str, key: &str) -> Option<(u64, u64)> {
    let rest = key.strip_prefix(prefix)?;
    let (e, c) = rest.split_once('/')?;
    let c = c.strip_suffix(".jsonl")?;
    if e.len() < 6 || c.len() < 8 || !e.bytes().chain(c.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((e.parse().ok()?, c.parse().ok()?))
}

/// A chunk's first line.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkHeader {
    /// Format version (1).
    pub h: u32,
    pub epoch: u64,
    pub index: u64,
    /// Byte offset of each line file after this chunk.
    #[serde(default)]
    pub end: BTreeMap<String, u64>,
    /// On the first chunk of an epoch: where each line file of this drive
    /// starts (what it had locally; at most what earlier epochs shipped).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub start: BTreeMap<String, u64>,
    #[serde(default)]
    pub ts_ms: u64,
}

/// One record of a chunk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkRecord {
    /// The journal file.
    pub f: String,
    /// Byte offset of the line in the local file (absent for a snapshot).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub off: Option<u64>,
    /// The line without its newline, or the whole snapshot.
    pub l: String,
}

/// Where shipping stands: the last chunk on the Volume.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    /// The epoch of the last chunk (0: none yet).
    pub epoch: u64,
    /// The last chunk's index in that epoch (`None`: no chunk).
    pub index: Option<u64>,
    /// Byte offsets shipped per line file.
    pub end: BTreeMap<String, u64>,
}

impl Cursor {
    /// The next chunk index for `epoch`.
    pub fn next_index(&self, epoch: u64) -> u64 {
        match self.index {
            Some(i) if self.epoch == epoch => i + 1,
            _ => 0,
        }
    }

    /// Shipped bytes of `file`.
    pub fn end_of(&self, file: &str) -> u64 {
        self.end.get(file).copied().unwrap_or(0)
    }
}

/// One journal line to ship.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub file: String,
    pub off: u64,
    /// Without the newline.
    pub bytes: Vec<u8>,
}

/// What one shipping step carries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    /// Where this drive's line files start, used only by the first chunk of
    /// an epoch (default: where the cursor says shipping ended). A re-drive
    /// from a restored journal starts where the restore ended.
    pub start: BTreeMap<String, u64>,
    pub lines: Vec<Line>,
    /// A new `state.json` snapshot.
    pub state: Option<Vec<u8>>,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.state.is_none()
    }
}

/// The fence object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fence {
    pub epoch: u64,
    #[serde(default)]
    pub holder: String,
    #[serde(default)]
    pub ts_ms: u64,
}

/// The error a writer below the fence gets (`precondition_failed`).
pub fn fenced(floor: &Fence, epoch: u64) -> Error {
    Error::Precondition(format!(
        "fenced: the session is at epoch {} (held by {}); epoch {epoch} may not write",
        floor.epoch,
        if floor.holder.is_empty() {
            "another driver"
        } else {
            &floor.holder
        }
    ))
}

/// The fence and its etag, when one is set.
pub async fn read_fence(b: &dyn Backend, prefix: &str) -> Result<Option<(Fence, String)>> {
    let key = format!("{prefix}{FENCE}");
    match b.get(&key, None).await {
        Ok((bytes, m)) => {
            let f: Fence = serde_json::from_slice(&bytes)
                .map_err(|e| Error::Backend(format!("{key}: {e}")))?;
            Ok(Some((f, m.etag)))
        }
        Err(Error::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Raises the fence to `epoch` for `holder` (compare-and-swap). The same
/// holder raising to the epoch it holds is a no-op. Fails with
/// [`fenced`] when the floor is above `epoch`, or at `epoch` for another
/// holder (two drivers were handed the same epoch: one loses).
pub async fn raise_fence(b: &dyn Backend, prefix: &str, epoch: u64, holder: &str) -> Result<Fence> {
    let key = format!("{prefix}{FENCE}");
    // Bounded: each retry follows a write someone else landed.
    for _ in 0..16 {
        let cur = read_fence(b, prefix).await?;
        let cond = match &cur {
            Some((f, _)) if f.epoch > epoch || (f.epoch == epoch && f.holder != holder) => {
                return Err(fenced(f, epoch));
            }
            Some((f, _)) if f.epoch == epoch => return Ok(f.clone()),
            Some((_, etag)) => Condition::IfMatch(etag.clone()),
            None => Condition::IfNoneMatch,
        };
        let next = Fence {
            epoch,
            holder: holder.to_string(),
            ts_ms: now_ms(),
        };
        match b.put(&key, serde_json::to_vec(&next)?, cond).await {
            Ok(_) => return Ok(next),
            Err(Error::Precondition(_)) => continue,
            Err(e) => return Err(e),
        }
    }
    Err(Error::Precondition(format!(
        "{key} kept changing while epoch {epoch} was raising it"
    )))
}

/// Refuses a writer of `epoch` below the fence.
pub async fn check_fence(b: &dyn Backend, prefix: &str, epoch: u64) -> Result<Option<Fence>> {
    match read_fence(b, prefix).await? {
        Some((f, _)) if f.epoch > epoch => Err(fenced(&f, epoch)),
        other => Ok(other.map(|(f, _)| f)),
    }
}

/// Every chunk key of the session, oldest first, with (epoch, index).
pub async fn chunks(b: &dyn Backend, prefix: &str) -> Result<Vec<(u64, u64, ObjectMeta)>> {
    let mut v: Vec<(u64, u64, ObjectMeta)> = b
        .list(prefix)
        .await?
        .into_iter()
        .filter_map(|m| parse_chunk_key(prefix, &m.key).map(|(e, i)| (e, i, m)))
        .collect();
    v.sort_by_key(|(e, i, _)| (*e, *i));
    Ok(v)
}

fn parse_chunk(key: &str, bytes: &[u8]) -> Result<(ChunkHeader, Vec<ChunkRecord>)> {
    let mut lines = bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty());
    let head: ChunkHeader = lines
        .next()
        .ok_or_else(|| Error::Backend(format!("{key}: empty chunk")))
        .and_then(|l| {
            serde_json::from_slice(l).map_err(|e| Error::Backend(format!("{key}: header: {e}")))
        })?;
    let mut recs = vec![];
    for l in lines {
        recs.push(
            serde_json::from_slice(l).map_err(|e| Error::Backend(format!("{key}: record: {e}")))?,
        );
    }
    Ok((head, recs))
}

/// The shipping cursor: the header of the newest chunk.
pub async fn cursor(b: &dyn Backend, prefix: &str) -> Result<Cursor> {
    let all = chunks(b, prefix).await?;
    let Some((epoch, index, m)) = all.last() else {
        return Ok(Cursor::default());
    };
    let (bytes, _) = b.get(&m.key, None).await?;
    let (h, _) = parse_chunk(&m.key, &bytes)?;
    Ok(Cursor {
        epoch: *epoch,
        index: Some(*index),
        end: h.end,
    })
}

/// Ships `batch` as the next chunk of `epoch` (create-only) and advances
/// `cur`. Refused below the fence, for an epoch older than the cursor's,
/// and for a line that is not where the cursor says the file ends (within
/// an epoch the chunks are contiguous; the first chunk of a new epoch may
/// start earlier, at what the new driver has locally). Returns the chunk's
/// metadata, or `None` for an empty batch.
pub async fn ship(
    b: &dyn Backend,
    prefix: &str,
    epoch: u64,
    cur: &mut Cursor,
    batch: Batch,
) -> Result<Option<ObjectMeta>> {
    if batch.is_empty() {
        return Ok(None);
    }
    check_fence(b, prefix, epoch).await?;
    if epoch < cur.epoch {
        return Err(Error::Precondition(format!(
            "epoch {epoch} is below the shipped epoch {}",
            cur.epoch
        )));
    }
    let index = cur.next_index(epoch);
    let mut end = cur.end.clone();
    let mut start = BTreeMap::new();
    if index == 0 {
        for f in LINE_FILES {
            let shipped = cur.end_of(f);
            let s = batch.start.get(f).copied().unwrap_or(shipped);
            if s > shipped {
                return Err(Error::Invalid(format!(
                    "{f} cannot start at {s}: only {shipped} bytes were shipped"
                )));
            }
            end.insert(f.to_string(), s);
            start.insert(f.to_string(), s);
        }
    }
    let mut body: Vec<u8> = vec![];
    let mut recs: Vec<u8> = vec![];
    for mut line in batch.lines {
        if !LINE_FILES.contains(&line.file.as_str()) {
            return Err(Error::Invalid(format!(
                "{} is not a journal file",
                line.file
            )));
        }
        if line.bytes.contains(&b'\n') {
            return Err(Error::Invalid("a journal line holds a newline".into()));
        }
        let at = end.get(&line.file).copied().unwrap_or(0);
        if line.off != at {
            return Err(Error::Invalid(format!(
                "{} line at {} does not follow the shipped end {at}",
                line.file, line.off
            )));
        }
        crate::scan::redact_line(&mut line.bytes);
        end.insert(line.file.clone(), line.off + line.bytes.len() as u64 + 1);
        let rec = ChunkRecord {
            f: line.file,
            off: Some(line.off),
            l: String::from_utf8_lossy(&line.bytes).into_owned(),
        };
        serde_json::to_writer(&mut recs, &rec)?;
        recs.push(b'\n');
    }
    if let Some(mut st) = batch.state {
        crate::scan::redact_line(&mut st);
        let rec = ChunkRecord {
            f: STATE_FILE.into(),
            off: None,
            l: String::from_utf8_lossy(&st).into_owned(),
        };
        serde_json::to_writer(&mut recs, &rec)?;
        recs.push(b'\n');
    }
    let head = ChunkHeader {
        h: 1,
        epoch,
        index,
        end: end.clone(),
        start,
        ts_ms: now_ms(),
    };
    serde_json::to_writer(&mut body, &head)?;
    body.push(b'\n');
    body.extend_from_slice(&recs);
    let key = chunk_key(prefix, epoch, index);
    let m = b
        .put(&key, body, Condition::IfNoneMatch)
        .await
        .map_err(|e| match e {
            Error::Precondition(_) => Error::Precondition(format!(
                "{key} already exists (another shipper landed it); read the cursor again"
            )),
            other => other,
        })?;
    cur.epoch = epoch;
    cur.index = Some(index);
    cur.end = end;
    Ok(Some(m))
}

/// A journal rebuilt from the Volume.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Restored {
    /// File name -> contents (`tools.jsonl`, `events.jsonl`, `state.json`,
    /// `epoch`); only the files the chunks had.
    pub files: BTreeMap<String, Vec<u8>>,
    /// The newest epoch shipped (0: nothing).
    pub epoch: u64,
    /// Chunks read.
    pub chunks: usize,
    /// Lines dropped: superseded by a later epoch, or after a gap.
    pub dropped: usize,
}

/// Rebuilds the journal of the session from its chunks (see the module
/// docs for the rule).
pub async fn restore(b: &dyn Backend, prefix: &str) -> Result<Restored> {
    let all = chunks(b, prefix).await?;
    let mut out = Restored::default();
    // (epoch, index, record)
    let mut recs: Vec<(u64, u64, ChunkRecord)> = vec![];
    // Where each epoch starts in each file.
    let mut base: BTreeMap<(String, u64), u64> = BTreeMap::new();
    for (epoch, index, m) in &all {
        let (bytes, _) = b.get(&m.key, None).await?;
        let (h, rs) = parse_chunk(&m.key, &bytes)?;
        for (f, off) in &h.start {
            base.insert((f.clone(), *epoch), *off);
        }
        recs.extend(rs.into_iter().map(|r| (*epoch, *index, r)));
        out.chunks += 1;
        out.epoch = out.epoch.max(*epoch);
    }
    if all.is_empty() {
        return Ok(out);
    }
    // An epoch without a start header (none is written without a first
    // chunk, but be lenient): its first line.
    for (e, _, r) in &recs {
        if let Some(off) = r.off {
            base.entry((r.f.clone(), *e)).or_insert(off);
        }
    }
    let cutoff = |f: &str, e: u64| -> u64 {
        base.iter()
            .filter(|((bf, be), _)| bf == f && *be > e)
            .map(|(_, off)| *off)
            .min()
            .unwrap_or(u64::MAX)
    };
    for file in LINE_FILES {
        let mut lines: BTreeMap<u64, (u64, &str)> = BTreeMap::new();
        for (e, _, r) in recs.iter().filter(|(_, _, r)| r.f == file) {
            let Some(off) = r.off else { continue };
            if off >= cutoff(file, *e) {
                out.dropped += 1;
                continue;
            }
            // The newest epoch's copy of an offset wins.
            match lines.get(&off) {
                Some((pe, _)) if *pe > *e => {}
                _ => {
                    lines.insert(off, (*e, r.l.as_str()));
                }
            }
        }
        if lines.is_empty() {
            continue;
        }
        let mut body: Vec<u8> = vec![];
        for (off, (_, l)) in &lines {
            let at = body.len() as u64;
            if *off < at {
                out.dropped += 1;
                continue;
            }
            if *off > at {
                // A gap: nothing after it can be placed.
                out.dropped += lines.range(off..).count();
                break;
            }
            body.extend_from_slice(l.as_bytes());
            body.push(b'\n');
        }
        out.files.insert(file.to_string(), body);
    }
    if let Some((_, _, r)) = recs.iter().rev().find(|(_, _, r)| r.f == STATE_FILE) {
        out.files
            .insert(STATE_FILE.to_string(), r.l.clone().into_bytes());
    }
    out.files
        .insert(EPOCH_FILE.to_string(), out.epoch.to_string().into_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FsBackend;

    fn backend() -> (tempfile::TempDir, FsBackend) {
        let d = tempfile::tempdir().unwrap();
        let b = FsBackend::new(d.path());
        (d, b)
    }

    fn line(file: &str, off: u64, s: &str) -> Line {
        Line {
            file: file.into(),
            off,
            bytes: s.as_bytes().to_vec(),
        }
    }

    /// Lines of `text` (each ending in a newline) from `from`.
    fn lines_of(file: &str, text: &str, from: u64) -> Vec<Line> {
        let mut off = 0u64;
        let mut out = vec![];
        for l in text.split_inclusive('\n') {
            if off >= from {
                out.push(line(file, off, l.trim_end_matches('\n')));
            }
            off += l.len() as u64;
        }
        out
    }

    #[test]
    fn prefixes_and_keys() {
        assert_eq!(
            session_prefix("local:dev", "run-0a1b2c3d").unwrap(),
            "spaces/local-dev/sessions/run-0a1b2c3d/"
        );
        assert!(session_prefix("local:dev", "../x").is_err());
        assert!(session_prefix("local:dev", "").is_err());
        let p = "spaces/x/sessions/s/";
        assert_eq!(
            chunk_key(p, 3, 12),
            "spaces/x/sessions/s/000003/00000012.jsonl"
        );
        assert_eq!(parse_chunk_key(p, &chunk_key(p, 3, 12)), Some((3, 12)));
        assert_eq!(parse_chunk_key(p, "spaces/x/sessions/s/fence"), None);
    }

    #[tokio::test]
    async fn chunks_are_create_only_and_ordered() {
        let (_d, b) = backend();
        let p = session_prefix("local:dev", "s1").unwrap();
        let mut cur = cursor(&b, &p).await.unwrap();
        assert_eq!(cur, Cursor::default());
        let m = ship(
            &b,
            &p,
            1,
            &mut cur,
            Batch {
                start: BTreeMap::new(),
                lines: vec![
                    line("tools.jsonl", 0, "{\"seq\":1}"),
                    line("events.jsonl", 0, "{\"seq\":1}"),
                ],
                state: Some(b"{\"status\":\"running\"}".to_vec()),
            },
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(m.key, chunk_key(&p, 1, 0));
        assert_eq!(cur.end_of("tools.jsonl"), 10);
        ship(
            &b,
            &p,
            1,
            &mut cur,
            Batch {
                start: BTreeMap::new(),
                lines: vec![line("events.jsonl", 10, "{\"seq\":2}")],
                state: None,
            },
        )
        .await
        .unwrap();
        // A line that skips ahead is refused (the chunks are contiguous).
        let e = ship(
            &b,
            &p,
            1,
            &mut cur.clone(),
            Batch {
                start: BTreeMap::new(),
                lines: vec![line("events.jsonl", 99, "{}")],
                state: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.tag(), "invalid_argument");
        // A second shipper with a stale cursor cannot overwrite chunk 1.
        let mut stale = Cursor {
            epoch: 1,
            index: Some(0),
            end: BTreeMap::from([("events.jsonl".into(), 10), ("tools.jsonl".into(), 10)]),
        };
        let e = ship(
            &b,
            &p,
            1,
            &mut stale,
            Batch {
                start: BTreeMap::new(),
                lines: vec![line("events.jsonl", 10, "{\"forged\":true}")],
                state: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.tag(), "precondition_failed");
        assert!(e.to_string().contains("already exists"), "{e}");
        // The cursor is read back from the newest header.
        let back = cursor(&b, &p).await.unwrap();
        assert_eq!(back, cur);
        assert_eq!(back.next_index(1), 2);
        assert_eq!(back.next_index(2), 0);
        let keys: Vec<(u64, u64)> = chunks(&b, &p)
            .await
            .unwrap()
            .into_iter()
            .map(|(e, i, _)| (e, i))
            .collect();
        assert_eq!(keys, [(1, 0), (1, 1)]);
        // Nothing to ship, nothing written.
        assert!(
            ship(&b, &p, 1, &mut cur, Batch::default())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn restore_equals_the_original_journal() {
        let (_d, b) = backend();
        let p = session_prefix("relay:abc", "run-00000001").unwrap();
        let tools = "{\"seq\":1,\"kind\":\"intent\",\"toolCallId\":\"t1\"}\n{\"seq\":2,\"kind\":\"outcome\",\"toolCallId\":\"t1\"}\n{\"seq\":3,\"kind\":\"intent\",\"toolCallId\":\"t2\"}\n";
        let events: String = (1..=50)
            .map(|i| format!("{{\"seq\":{i},\"type\":\"update\",\"text\":\"chunk {i} ✓\"}}\n"))
            .collect();
        let mut cur = Cursor::default();
        // Epoch 1 ships the first half in two chunks, epoch 2 (same host) the rest.
        let ev = lines_of("events.jsonl", &events, 0);
        let tl = lines_of("tools.jsonl", tools, 0);
        ship(
            &b,
            &p,
            1,
            &mut cur,
            Batch {
                start: BTreeMap::new(),
                lines: [tl[..1].to_vec(), ev[..10].to_vec()].concat(),
                state: None,
            },
        )
        .await
        .unwrap();
        ship(
            &b,
            &p,
            1,
            &mut cur,
            Batch {
                start: BTreeMap::new(),
                lines: ev[10..25].to_vec(),
                state: Some(b"{\"turn\":1}".to_vec()),
            },
        )
        .await
        .unwrap();
        ship(
            &b,
            &p,
            2,
            &mut cur,
            Batch {
                start: BTreeMap::new(),
                lines: [tl[1..].to_vec(), ev[25..].to_vec()].concat(),
                state: Some(b"{\"turn\":2}".to_vec()),
            },
        )
        .await
        .unwrap();
        let r = restore(&b, &p).await.unwrap();
        assert_eq!(r.chunks, 3);
        assert_eq!(r.epoch, 2);
        assert_eq!(r.dropped, 0);
        assert_eq!(
            String::from_utf8(r.files["events.jsonl"].clone()).unwrap(),
            events
        );
        assert_eq!(
            String::from_utf8(r.files["tools.jsonl"].clone()).unwrap(),
            tools
        );
        assert_eq!(r.files["state.json"], b"{\"turn\":2}");
        assert_eq!(r.files["epoch"], b"2");
        // Nothing shipped: nothing restored.
        let empty = restore(&b, &session_prefix("relay:abc", "none").unwrap())
            .await
            .unwrap();
        assert!(empty.files.is_empty() && empty.epoch == 0);
    }

    #[tokio::test]
    async fn a_later_epoch_supersedes_what_a_stale_driver_shipped() {
        let (_d, b) = backend();
        let p = session_prefix("s", "x").unwrap();
        let mut a = Cursor::default();
        ship(
            &b,
            &p,
            1,
            &mut a,
            Batch {
                start: BTreeMap::new(),
                lines: lines_of("events.jsonl", "a1\na2\n", 0),
                state: None,
            },
        )
        .await
        .unwrap();
        // Epoch 2 restores "a1\na2\n" on another host and appends b3.
        let mut c2 = cursor(&b, &p).await.unwrap();
        // Meanwhile the stale epoch-1 driver ships a3, just before the
        // re-driver raises the fence (a check-then-write race).
        ship(
            &b,
            &p,
            1,
            &mut a,
            Batch {
                start: BTreeMap::new(),
                lines: vec![line("events.jsonl", 6, "a3")],
                state: None,
            },
        )
        .await
        .unwrap();
        raise_fence(&b, &p, 2, "host-b").await.unwrap();
        let start = BTreeMap::from([("events.jsonl".to_string(), 6)]);
        ship(
            &b,
            &p,
            2,
            &mut c2,
            Batch {
                start,
                lines: vec![line("events.jsonl", 6, "b3")],
                state: None,
            },
        )
        .await
        .unwrap();
        let r = restore(&b, &p).await.unwrap();
        assert_eq!(r.files["events.jsonl"], b"a1\na2\nb3\n");
        assert_eq!(r.dropped, 1);
        // Behind the fence, epoch 1 can ship no more.
        let e = ship(
            &b,
            &p,
            1,
            &mut a,
            Batch {
                start: BTreeMap::new(),
                lines: vec![line("events.jsonl", 9, "a4")],
                state: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.tag(), "precondition_failed");
    }

    #[tokio::test]
    async fn the_fence_is_a_cas_and_the_loser_gets_the_exact_error() {
        let (_d, b) = backend();
        let p = session_prefix("local:dev", "s1").unwrap();
        assert!(read_fence(&b, &p).await.unwrap().is_none());
        let f = raise_fence(&b, &p, 3, "host-a").await.unwrap();
        assert_eq!(f.epoch, 3);
        // Idempotent for the holder.
        raise_fence(&b, &p, 3, "host-a").await.unwrap();
        // Two drivers handed epoch 3: the second loses.
        let e = raise_fence(&b, &p, 3, "host-b").await.unwrap_err();
        assert_eq!(e.tag(), "precondition_failed");
        assert_eq!(
            e.to_string(),
            "precondition failed: fenced: the session is at epoch 3 (held by host-a); epoch 3 may not write"
        );
        // Two writers racing from the same floor: both read epoch 3, one CAS wins.
        let (r4, r5) = tokio::join!(
            raise_fence(&b, &p, 4, "host-b"),
            raise_fence(&b, &p, 5, "host-c")
        );
        let floor = read_fence(&b, &p).await.unwrap().unwrap().0;
        assert_eq!(floor.epoch, 5, "the higher epoch always ends on top");
        r5.unwrap();
        if let Err(e) = r4 {
            assert_eq!(
                e.to_string(),
                "precondition failed: fenced: the session is at epoch 5 (held by host-c); epoch 4 may not write"
            );
        }
        // A lower epoch can neither raise nor ship.
        let e = raise_fence(&b, &p, 4, "host-b").await.unwrap_err();
        assert!(e.to_string().contains("epoch 4 may not write"), "{e}");
        let mut cur = Cursor::default();
        let e = ship(
            &b,
            &p,
            4,
            &mut cur,
            Batch {
                start: BTreeMap::new(),
                lines: vec![line("events.jsonl", 0, "x")],
                state: None,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.tag(), "precondition_failed");
        assert!(
            ship(
                &b,
                &p,
                5,
                &mut cur,
                Batch {
                    start: BTreeMap::new(),
                    lines: vec![line("events.jsonl", 0, "x")],
                    state: None
                }
            )
            .await
            .unwrap()
            .is_some()
        );
    }

    #[tokio::test]
    async fn secrets_never_reach_the_volume() {
        let (_d, b) = backend();
        let p = session_prefix("s", "x").unwrap();
        let key = format!("sk-{}-{}", "ant", "api03-ABCDEFGHIJKLMNOPQRSTUVWXYZabcd");
        let l = serde_json::json!({"seq": 1, "text": format!("export KEY={key}")}).to_string();
        let mut cur = Cursor::default();
        ship(
            &b,
            &p,
            1,
            &mut cur,
            Batch {
                start: BTreeMap::new(),
                lines: vec![line("events.jsonl", 0, &l)],
                state: Some(format!("{{\"error\":\"{key}\"}}").into_bytes()),
            },
        )
        .await
        .unwrap();
        let (_, _, m) = chunks(&b, &p).await.unwrap().pop().unwrap();
        let (raw, _) = b.get(&m.key, None).await.unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("sk-ant"));
        assert!(crate::scan::scan(&raw).is_empty());
        let r = restore(&b, &p).await.unwrap();
        // Same length: the offsets after it still hold.
        assert_eq!(r.files["events.jsonl"].len(), l.len() + 1);
        assert_eq!(cur.end_of("events.jsonl"), l.len() as u64 + 1);
    }
}
