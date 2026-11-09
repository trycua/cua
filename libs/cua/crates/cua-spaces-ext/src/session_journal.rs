// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The host side of durable session journals: ships a durable run's
//! journal from its state directory in the Space to the Cua Volume
//! ([`cua_volume::journal`]), acknowledges it to the runner, and rebuilds a
//! state directory from the Volume for a re-drive elsewhere.
//!
//! The runner runs inside the Space and has no write access to
//! `spaces/<S>/sessions/` (an agent must not rewrite its own history), so
//! the host that drives the run (the persistent-agent supervisor) is the
//! writer: each pass it reads the new journal lines through spacesd, ships
//! them as one immutable chunk, and writes `ship.ack` into the state
//! directory. The runner answers a permission request only after the ack
//! covers its intent ([`cua_spaces::agents::durable::Ship`]).
//!
//! When the Volume fence is above this shipper's epoch (another host
//! re-drove the session), the shipper writes the floor into the local
//! `epoch` file: the runner polls it and stops with exit code 75.

use std::collections::BTreeMap;
use std::sync::Arc;

use cua_spaces::{Error, Result};
use cua_spacesd_client::{DownloadOptions, SpacesdClient, UploadOptions, pb};
use cua_volume::Backend;
use cua_volume::journal::{self, Batch, Cursor, Line};

use crate::DriveResult as _;

/// The file the shipper writes for the runner (`durable::SHIP_ACK_FILE`).
pub const ACK_FILE: &str = cua_agents::durable::SHIP_ACK_FILE;

/// A durable run's state directory, wherever it is.
#[async_trait::async_trait]
pub trait JournalDir: Send + Sync {
    /// The size of `file`, or `None` when it does not exist.
    async fn size(&self, file: &str) -> Result<Option<u64>>;
    /// Bytes `[off, off + len)` of `file` (short at the end).
    async fn read(&self, file: &str, off: u64, len: u64) -> Result<Vec<u8>>;
    /// Replaces `file`.
    async fn write(&self, file: &str, bytes: Vec<u8>) -> Result<()>;
}

/// A state directory in a Space, through its spacesd.
pub struct GuestDir {
    pub guest: SpacesdClient,
    /// Absolute guest path.
    pub dir: String,
}

impl GuestDir {
    fn path(&self, file: &str) -> String {
        format!("{}/{file}", self.dir.trim_end_matches('/'))
    }
}

#[async_trait::async_trait]
impl JournalDir for GuestDir {
    async fn size(&self, file: &str) -> Result<Option<u64>> {
        match self.guest.stat(&self.path(file)).await {
            Ok(e) => Ok(Some(e.size)),
            Err(cua_spacesd_client::Error::PathNotFound(_)) => Ok(None),
            Err(e) => Err(Error::Env(e)),
        }
    }

    async fn read(&self, file: &str, off: u64, len: u64) -> Result<Vec<u8>> {
        let mut buf = vec![];
        self.guest
            .download_with(
                &self.path(file),
                DownloadOptions {
                    offset: off,
                    length: len,
                    ..Default::default()
                },
                &mut buf,
            )
            .await
            .map_err(Error::Env)?;
        Ok(buf)
    }

    async fn write(&self, file: &str, bytes: Vec<u8>) -> Result<()> {
        self.guest
            .upload(
                &self.path(file),
                bytes,
                UploadOptions {
                    mode: pb::WriteMode::Overwrite,
                    create_parents: true,
                    permissions: 0o600,
                    ..Default::default()
                },
            )
            .await
            .map_err(Error::Env)?;
        Ok(())
    }
}

/// What one shipping pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShipReport {
    /// The chunk written, if any.
    pub chunk: Option<String>,
    /// Journal lines in it.
    pub lines: usize,
    /// The Volume fence is above this epoch: the local runner was fenced
    /// (its `epoch` file now holds this floor) and nothing was shipped.
    pub fenced: Option<u64>,
}

/// Ships one durable session's journal, pass by pass. Keep one per
/// (session, epoch) for the life of the drive; a new one (after a daemon
/// restart) picks up where the Volume says shipping stopped.
pub struct Shipper {
    backend: Arc<dyn Backend>,
    prefix: String,
    epoch: u64,
    cur: Option<Cursor>,
    /// Local bytes shipped per line file.
    pos: BTreeMap<String, u64>,
    last_state: Option<Vec<u8>>,
    acked: Option<BTreeMap<String, u64>>,
    /// Most bytes read per file per pass.
    max_bytes: usize,
}

impl Shipper {
    /// A shipper of `session` in the Space folder of `space` (a Space id or
    /// a durable Space's stable name) for `epoch`.
    pub fn new(backend: Arc<dyn Backend>, space: &str, session: &str, epoch: u64) -> Result<Self> {
        Ok(Shipper {
            backend,
            prefix: journal::session_prefix(space, session).drive()?,
            epoch,
            cur: None,
            pos: BTreeMap::new(),
            last_state: None,
            acked: None,
            max_bytes: journal::MAX_CHUNK_BYTES,
        })
    }

    /// The session's folder on the Volume.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// This drive's epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Caps the bytes read per file per pass (tests).
    pub fn max_bytes(mut self, n: usize) -> Self {
        self.max_bytes = n.max(1);
        self
    }

    /// One pass: fence check, new lines and a changed state snapshot as one
    /// chunk, then the ack. Bounded by the per-pass byte cap; call it again
    /// for the rest.
    pub async fn tick(&mut self, dir: &dyn JournalDir) -> Result<ShipReport> {
        let b = self.backend.as_ref();
        match journal::check_fence(b, &self.prefix, self.epoch).await {
            Ok(_) => {}
            Err(cua_volume::Error::Precondition(_)) => {
                let floor = journal::read_fence(b, &self.prefix)
                    .await
                    .drive()?
                    .map(|(f, _)| f.epoch)
                    .unwrap_or(self.epoch + 1);
                dir.write(journal::EPOCH_FILE, floor.to_string().into_bytes())
                    .await?;
                return Ok(ShipReport {
                    fenced: Some(floor),
                    ..Default::default()
                });
            }
            Err(e) => return Err(crate::drive_err(e)),
        }
        if self.cur.is_none() {
            let cur = journal::cursor(b, &self.prefix).await.drive()?;
            for f in journal::LINE_FILES {
                let local = dir.size(f).await?.unwrap_or(0);
                // A restored state directory may be shorter than what was
                // shipped (a stale suffix the restore dropped): start there.
                self.pos.insert(f.to_string(), cur.end_of(f).min(local));
            }
            self.cur = Some(cur);
        }
        let mut batch = Batch {
            start: self.pos.clone(),
            ..Default::default()
        };
        let mut new_pos: BTreeMap<String, u64> = BTreeMap::new();
        for f in journal::LINE_FILES {
            let at = self.pos.get(f).copied().unwrap_or(0);
            let size = dir.size(f).await?.unwrap_or(0);
            if size <= at {
                continue;
            }
            let want = (size - at).min(self.max_bytes as u64);
            let bytes = dir.read(f, at, want).await?;
            let Some(last_nl) = bytes.iter().rposition(|c| *c == b'\n') else {
                if bytes.len() as u64 >= self.max_bytes as u64 {
                    return Err(Error::invalid(format!(
                        "{f}: a journal line longer than {} bytes",
                        self.max_bytes
                    )));
                }
                // A line still being written.
                continue;
            };
            let mut off = at;
            for l in bytes[..last_nl].split(|c| *c == b'\n') {
                batch.lines.push(Line {
                    file: f.to_string(),
                    off,
                    bytes: l.to_vec(),
                });
                off += l.len() as u64 + 1;
            }
            new_pos.insert(f.to_string(), off);
        }
        if let Some(n) = dir.size(journal::STATE_FILE).await?
            && n > 0
            && n < 1024 * 1024
        {
            let st = dir.read(journal::STATE_FILE, 0, n).await?;
            if self.last_state.as_deref() != Some(&st[..]) {
                batch.state = Some(st);
            }
        }
        let lines = batch.lines.len();
        let state = batch.state.clone();
        let cur = self.cur.as_mut().expect("set above");
        let chunk = match journal::ship(b, &self.prefix, self.epoch, cur, batch).await {
            Ok(m) => m.map(|m| m.key),
            Err(e) => {
                // Read the cursor again next pass (another shipper may have
                // landed a chunk; or the fence moved).
                self.cur = None;
                return Err(crate::drive_err(e));
            }
        };
        if let Some(st) = state {
            self.last_state = Some(st);
        }
        self.pos.extend(new_pos);
        if self.acked.as_ref() != Some(&self.pos) {
            let ack = serde_json::json!({"epoch": self.epoch, "end": self.pos});
            dir.write(ACK_FILE, serde_json::to_vec(&ack)?).await?;
            self.acked = Some(self.pos.clone());
        }
        Ok(ShipReport {
            chunk,
            lines,
            fenced: None,
        })
    }
}

/// Rebuilds a state directory from the Volume (see
/// [`cua_volume::journal::restore`]): writes the journal files into `dir`
/// and returns what was restored. Nothing shipped: nothing written.
pub async fn restore_into(
    backend: &dyn Backend,
    space: &str,
    session: &str,
    dir: &dyn JournalDir,
) -> Result<journal::Restored> {
    let r = restore(backend, space, session).await?;
    for (name, body) in &r.files {
        dir.write(name, body.clone()).await?;
    }
    Ok(r)
}

/// The journal of `session` as the Volume has it (for
/// [`cua_spaces::agents::durable::DurableOptions::restore`]).
pub async fn restore(
    backend: &dyn Backend,
    space: &str,
    session: &str,
) -> Result<journal::Restored> {
    let prefix = journal::session_prefix(space, session).drive()?;
    journal::restore(backend, &prefix).await.drive()
}

/// Raises the session's Volume fence to `epoch` for `holder` before a
/// re-drive (see [`cua_volume::journal::raise_fence`]).
pub async fn raise_fence(
    backend: &dyn Backend,
    space: &str,
    session: &str,
    epoch: u64,
    holder: &str,
) -> Result<journal::Fence> {
    let prefix = journal::session_prefix(space, session).drive()?;
    journal::raise_fence(backend, &prefix, epoch, holder)
        .await
        .drive()
}

/// An in-memory state directory (tests, and a staging area for a restore).
#[derive(Default)]
pub struct MemDir(pub std::sync::Mutex<BTreeMap<String, Vec<u8>>>);

impl MemDir {
    /// Appends to `file`.
    pub fn append(&self, file: &str, bytes: &[u8]) {
        self.0
            .lock()
            .expect("memdir")
            .entry(file.to_string())
            .or_default()
            .extend_from_slice(bytes);
    }

    /// The contents of `file`.
    pub fn get(&self, file: &str) -> Option<Vec<u8>> {
        self.0.lock().expect("memdir").get(file).cloned()
    }
}

#[async_trait::async_trait]
impl JournalDir for MemDir {
    async fn size(&self, file: &str) -> Result<Option<u64>> {
        Ok(self.get(file).map(|b| b.len() as u64))
    }

    async fn read(&self, file: &str, off: u64, len: u64) -> Result<Vec<u8>> {
        let b = self.get(file).unwrap_or_default();
        let s = (off as usize).min(b.len());
        let e = s.saturating_add(len as usize).min(b.len());
        Ok(b[s..e].to_vec())
    }

    async fn write(&self, file: &str, bytes: Vec<u8>) -> Result<()> {
        self.0
            .lock()
            .expect("memdir")
            .insert(file.to_string(), bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_volume::{Condition, ObjectMeta, VersionInfo};
    use std::sync::Mutex;
    use std::time::Duration;

    /// A Volume whose writes land late, and that logs when each one did.
    struct SlowVolume {
        inner: cua_volume::fs::FsBackend,
        delay: Duration,
        log: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl Backend for SlowVolume {
        fn kind(&self) -> &'static str {
            "slow"
        }
        async fn put(
            &self,
            key: &str,
            bytes: Vec<u8>,
            cond: Condition,
        ) -> cua_volume::Result<ObjectMeta> {
            tokio::time::sleep(self.delay).await;
            let m = self.inner.put(key, bytes, cond).await?;
            self.log.lock().unwrap().push(format!("volume {key}"));
            Ok(m)
        }
        async fn get(
            &self,
            key: &str,
            v: Option<&str>,
        ) -> cua_volume::Result<(Vec<u8>, ObjectMeta)> {
            self.inner.get(key, v).await
        }
        async fn head(&self, key: &str) -> cua_volume::Result<Option<ObjectMeta>> {
            self.inner.head(key).await
        }
        async fn list(&self, prefix: &str) -> cua_volume::Result<Vec<ObjectMeta>> {
            self.inner.list(prefix).await
        }
        async fn delete(&self, key: &str, cond: Condition) -> cua_volume::Result<()> {
            self.inner.delete(key, cond).await
        }
        async fn versions(&self, key: &str) -> cua_volume::Result<Vec<VersionInfo>> {
            self.inner.versions(key).await
        }
    }

    /// A state directory that logs its writes.
    struct LoggedDir {
        mem: MemDir,
        log: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl JournalDir for LoggedDir {
        async fn size(&self, f: &str) -> Result<Option<u64>> {
            self.mem.size(f).await
        }
        async fn read(&self, f: &str, off: u64, len: u64) -> Result<Vec<u8>> {
            self.mem.read(f, off, len).await
        }
        async fn write(&self, f: &str, b: Vec<u8>) -> Result<()> {
            self.log.lock().unwrap().push(format!("dir {f}"));
            self.mem.write(f, b).await
        }
    }

    fn ack(dir: &MemDir) -> serde_json::Value {
        serde_json::from_slice(&dir.get(ACK_FILE).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn the_ack_follows_the_chunk_and_covers_the_intent() {
        let d = tempfile::tempdir().unwrap();
        let log = Arc::new(Mutex::new(vec![]));
        let b: Arc<dyn Backend> = Arc::new(SlowVolume {
            inner: cua_volume::fs::FsBackend::new(d.path()),
            delay: Duration::from_millis(150),
            log: log.clone(),
        });
        let dir = LoggedDir {
            mem: MemDir::default(),
            log: log.clone(),
        };
        let intent = "{\"seq\":1,\"kind\":\"intent\",\"toolCallId\":\"t1\"}\n";
        dir.mem.append("tools.jsonl", intent.as_bytes());
        dir.mem.append(
            "events.jsonl",
            b"{\"seq\":1,\"type\":\"permission\"}\n{\"seq\":2,\"type\":\"upd",
        );
        let mut s = Shipper::new(b.clone(), "local:dev", "run-00000001", 1).unwrap();
        let r = s.tick(&dir).await.unwrap();
        assert_eq!(r.lines, 2, "the torn last event waits for its newline");
        assert_eq!(
            r.chunk.as_deref(),
            Some("spaces/local-dev/sessions/run-00000001/000001/00000000.jsonl")
        );
        let a = ack(&dir.mem);
        assert_eq!(a["epoch"], 1);
        assert_eq!(a["end"]["tools.jsonl"], intent.len() as u64);
        assert_eq!(
            a["end"]["events.jsonl"],
            "{\"seq\":1,\"type\":\"permission\"}\n".len() as u64
        );
        // The chunk was on the Volume before the runner could see the ack.
        assert_eq!(
            *log.lock().unwrap(),
            [
                "volume spaces/local-dev/sessions/run-00000001/000001/00000000.jsonl",
                "dir ship.ack"
            ]
        );
        // Nothing new: no chunk, no second ack.
        let r = s.tick(&dir).await.unwrap();
        assert_eq!((r.chunk, r.lines), (None, 0));
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_drive_elsewhere_restores_the_journal_and_continues_it() {
        let d = tempfile::tempdir().unwrap();
        let b: Arc<dyn Backend> = Arc::new(cua_volume::fs::FsBackend::new(d.path()));
        // Host A, epoch 1, ships in small passes.
        let a = MemDir::default();
        let mut events = String::new();
        for i in 1..=30 {
            events.push_str(&format!("{{\"seq\":{i},\"type\":\"update\"}}\n"));
        }
        a.append("events.jsonl", events.as_bytes());
        a.append(
            "tools.jsonl",
            b"{\"seq\":1,\"kind\":\"intent\",\"toolCallId\":\"t1\"}\n",
        );
        a.write(
            "state.json",
            b"{\"status\":\"running\",\"turn\":1}".to_vec(),
        )
        .await
        .unwrap();
        let mut sa = Shipper::new(b.clone(), "dev", "s1", 1)
            .unwrap()
            .max_bytes(200);
        for _ in 0..20 {
            if sa.tick(&a).await.unwrap().lines == 0 {
                break;
            }
        }
        // Host B re-drives at epoch 2 from the Volume.
        raise_fence(b.as_ref(), "dev", "s1", 2, "host-b")
            .await
            .unwrap();
        let bdir = MemDir::default();
        let r = restore_into(b.as_ref(), "dev", "s1", &bdir).await.unwrap();
        assert_eq!(bdir.get("events.jsonl").unwrap(), events.as_bytes());
        assert_eq!(
            bdir.get("tools.jsonl").unwrap(),
            a.get("tools.jsonl").unwrap()
        );
        assert_eq!(
            bdir.get("state.json").unwrap(),
            a.get("state.json").unwrap()
        );
        assert_eq!(bdir.get("epoch").unwrap(), b"1");
        assert_eq!(r.epoch, 1);
        // Host A's shipper is now below the fence: it fences its runner.
        a.append("events.jsonl", b"{\"seq\":31,\"type\":\"stale\"}\n");
        let fenced = sa.tick(&a).await.unwrap();
        assert_eq!(fenced.fenced, Some(2));
        assert_eq!(a.get("epoch").unwrap(), b"2");
        // B appends and ships at epoch 2; a later restore has both.
        bdir.append("events.jsonl", b"{\"seq\":31,\"type\":\"resumed\"}\n");
        let mut sb = Shipper::new(b.clone(), "dev", "s1", 2).unwrap();
        assert_eq!(sb.tick(&bdir).await.unwrap().lines, 1);
        let again = MemDir::default();
        restore_into(b.as_ref(), "dev", "s1", &again).await.unwrap();
        assert_eq!(again.get("events.jsonl"), bdir.get("events.jsonl"));
        assert_eq!(again.get("epoch").unwrap(), b"2");
    }
}
