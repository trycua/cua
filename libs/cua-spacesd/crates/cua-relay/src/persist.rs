// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! State files written off the request path (CUA-4413).
//!
//! On 23 Nov 2026 the relay's node paused its disk I/O for about a minute.
//! The stores wrote and fsynced their files inside request handlers while
//! holding the mutexes those handlers (and every reader) take, so each
//! tokio worker that touched a store blocked behind the stalled fsync. With
//! every worker blocked the runtime stopped: outbound calls to the Spaces
//! API, the OIDC issuer and PostHog all timed out at once and hundreds of
//! requests queued.
//!
//! A [`StateFile`] owns one file and a dedicated writer thread:
//!
//! - **Snapshot, then release.** A store changes its state under its lock,
//!   hands the writer an encoder that owns a snapshot of it
//!   ([`StateFile::replace`]) and lets go of the lock. The write and the
//!   fsyncs happen on the writer thread, never under a store lock and never
//!   on an async worker.
//! - **Latest state wins.** A snapshot queued while an older one waits
//!   replaces it, so a burst of changes during a stall costs one write when
//!   the disk answers again, and the queue holds at most one snapshot.
//! - **Journals.** [`StateFile::append`] queues lines for an append-only
//!   file, written and `fdatasync`ed in one batch. A queued snapshot of the
//!   whole journal supersedes the lines queued before it. The lines waiting
//!   are bounded by [`MAX_QUEUED_APPEND_BYTES`]; past it (or after an
//!   append failed, which leaves the file short of what memory holds) the
//!   writer refuses lines with [`NeedsRewrite`] until the store queues a
//!   full snapshot, so the file never has a gap and memory stays bounded.
//! - **Durability is awaited, not blocked on.** Each change gets a
//!   [`Durable`]: a future that resolves with the outcome of the write that
//!   covers it. A request that must not answer before its change is on
//!   disk awaits it (yielding the worker); background state ignores it.
//! - **Atomic files.** Snapshots go through
//!   [`crate::assertion::write_private_with`]: a fresh 0600 temp file,
//!   fsynced, renamed over the file, directory fsynced.
//! - **Visible when behind.** Every write is reported to [`Storage`]
//!   (`/readyz`, `/metrics`): its outcome, its duration
//!   (`cua_relay_state_write_duration_seconds`), how long the oldest
//!   unwritten change has waited (`cua_relay_state_writer_behind_seconds`)
//!   and how many snapshots were coalesced or journal batches dropped for a
//!   rewrite. A write slower than [`SLOW_WRITE`] is logged
//!   (`state write slow`).

use std::collections::BTreeMap;
use std::future::Future;
use std::io::{self, Write};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crate::storage::Storage;

/// Writes one snapshot of a state file's content.
pub type Encode = Box<dyn FnOnce(&mut dyn Write) -> io::Result<()> + Send>;

/// Most bytes of journal lines a [`StateFile`] queues while its writer is
/// busy. Past this the queued lines are dropped and the store rewrites the
/// whole journal from memory instead ([`NeedsRewrite`]).
pub const MAX_QUEUED_APPEND_BYTES: usize = 8 << 20;

/// How long dropping a [`StateFile`] waits for its queued writes.
pub const DROP_FLUSH: Duration = Duration::from_secs(10);

/// A write slower than this is logged (`state write slow`).
pub const SLOW_WRITE: Duration = Duration::from_secs(5);

/// The error a [`Durable`] resolves with when the write that covers it
/// failed or the writer stopped.
#[derive(Debug, Clone)]
pub struct WriteError(Arc<str>);

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for WriteError {}

impl From<WriteError> for io::Error {
    fn from(e: WriteError) -> Self {
        io::Error::other(e.0.to_string())
    }
}

/// [`StateFile::append`] refused the line: the journal must be rewritten
/// whole from memory ([`StateFile::replace`]) first. Either a write failed
/// (the file may lack lines memory has) or the writer fell too far behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeedsRewrite;

/// A write's outcome once known, and the task waiting for it.
type SlotState = (Option<Result<(), WriteError>>, Option<Waker>);

/// A completion slot one [`Durable`] waits on.
#[derive(Default)]
struct Slot {
    state: Mutex<SlotState>,
    done: Condvar,
}

impl Slot {
    fn complete(&self, result: Result<(), WriteError>) {
        let waker = {
            let mut s = self.state.lock().expect("durable slot");
            if s.0.is_none() {
                s.0 = Some(result);
            }
            s.1.take()
        };
        self.done.notify_all();
        if let Some(w) = waker {
            w.wake();
        }
    }
}

/// The outcome of the write that makes one change durable. Await it (it
/// never blocks a worker), or ignore it for state whose writes are best
/// effort. [`Durable::wait_blocking`] is for code off the async runtime.
#[must_use = "await it when the caller must not answer before the change is on disk"]
pub struct Durable(Option<Arc<Slot>>);

impl Durable {
    /// Nothing to write (an in-memory store, or no change).
    pub fn ready() -> Self {
        Self(None)
    }

    /// Lets the write go on without waiting for it: state whose writes are
    /// best effort (a failure is counted and logged by the writer, and the
    /// next write retries it).
    pub fn detach(self) {}

    /// Blocks the calling thread until the write is done. Never call this
    /// on an async worker.
    pub fn wait_blocking(self) -> Result<(), WriteError> {
        let Some(slot) = self.0 else { return Ok(()) };
        let mut s = slot.state.lock().expect("durable slot");
        loop {
            if let Some(r) = s.0.take() {
                return r;
            }
            s = slot.done.wait(s).expect("durable slot");
        }
    }
}

impl Future for Durable {
    type Output = Result<(), WriteError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Some(slot) = self.0.clone() else {
            return Poll::Ready(Ok(()));
        };
        let mut s = slot.state.lock().expect("durable slot");
        if let Some(r) = s.0.take() {
            drop(s);
            self.0 = None;
            return Poll::Ready(r);
        }
        s.1 = Some(cx.waker().clone());
        Poll::Pending
    }
}

/// What waits for the writer.
#[derive(Default)]
struct Queue {
    /// The last change number handed out.
    seq: u64,
    /// The newest whole-file snapshot not yet taken by the writer.
    replace: Option<Encode>,
    /// Journal lines queued after `replace` (or after the file as written).
    appends: Vec<u8>,
    /// Lines must not be appended until a snapshot is written.
    needs_rewrite: bool,
    /// Changes waiting for the write that covers them.
    waiters: Vec<(u64, Arc<Slot>)>,
    /// When the oldest change not yet on disk was queued.
    pending_since: Option<Instant>,
    /// The writer is writing a batch.
    busy: bool,
    /// The last write succeeded (true before any).
    last_ok: bool,
    /// The [`StateFile`] was dropped: write what is queued, then stop.
    closed: bool,
    /// The writer thread was started.
    started: bool,
    /// Tests: the writer holds its next batch as a stalled disk would.
    #[cfg(test)]
    stalled: bool,
}

impl Queue {
    fn has_work(&self) -> bool {
        self.replace.is_some() || !self.appends.is_empty()
    }

    /// Resolves the waiters of changes up to `upto`.
    fn complete(&mut self, upto: u64, result: &Result<(), WriteError>) {
        let (done, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.waiters)
            .into_iter()
            .partition(|(seq, _)| *seq <= upto);
        self.waiters = rest;
        for (_, slot) in done {
            slot.complete(result.clone());
        }
    }
}

struct Shared {
    path: Option<PathBuf>,
    store: &'static str,
    storage: Arc<Storage>,
    /// Distinguishes this writer in [`Storage`]'s behind gauge.
    id: u64,
    queue: Mutex<Queue>,
    /// Wakes the writer (work queued, closed, stall lifted).
    wake: Condvar,
    /// Wakes [`StateFile::flush_blocking`] (the writer went idle).
    idle: Condvar,
}

impl Shared {
    fn flush_until(&self, deadline: Instant) -> bool {
        let mut q = self.queue.lock().expect("state file");
        while q.has_work() || q.busy {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            q = self.idle.wait_timeout(q, left).expect("state file").0;
        }
        true
    }
}

/// A writer [`Storage::flush_writers`] can wait for at shutdown.
pub(crate) trait Flush: Send + Sync {
    /// Waits until the writer caught up or `deadline` passed; whether it
    /// caught up.
    fn flush_until(&self, deadline: Instant) -> bool;
}

impl Flush for Shared {
    fn flush_until(&self, deadline: Instant) -> bool {
        Shared::flush_until(self, deadline)
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// One state file and its writer thread. Dropping it lets the writer
/// finish what is queued and stop.
pub struct StateFile {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for StateFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StateFile")
            .field("path", &self.shared.path)
            .field("store", &self.shared.store)
            .finish()
    }
}

impl Default for StateFile {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl StateFile {
    /// A file at `path` (`None`: nothing is written and every change is
    /// durable at once), reported to `storage` as `store`.
    pub fn new(path: Option<PathBuf>, store: &'static str, storage: Arc<Storage>) -> Self {
        let persisted = path.is_some();
        let shared = Arc::new(Shared {
            path,
            store,
            storage: storage.clone(),
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            queue: Mutex::new(Queue {
                last_ok: true,
                ..Queue::default()
            }),
            wake: Condvar::new(),
            idle: Condvar::new(),
        });
        if persisted {
            let weak: std::sync::Weak<dyn Flush> = Arc::downgrade(&shared) as _;
            storage.register_writer(weak);
        }
        Self { shared }
    }

    /// Nothing persisted.
    pub fn in_memory() -> Self {
        Self::new(None, "memory", Arc::default())
    }

    /// Whether changes are written to a file.
    pub fn is_persisted(&self) -> bool {
        self.shared.path.is_some()
    }

    /// Whether the last write succeeded (true before any write and for an
    /// in-memory file).
    pub fn last_ok(&self) -> bool {
        self.shared.queue.lock().expect("state file").last_ok
    }

    /// Queues `encode` as the file's whole new content, superseding any
    /// snapshot or journal lines still waiting. Never blocks on the disk:
    /// call it under the store's lock so snapshots queue in the order of
    /// the changes, and await the result after releasing the lock.
    pub fn replace(
        &self,
        encode: impl FnOnce(&mut dyn Write) -> io::Result<()> + Send + 'static,
    ) -> Durable {
        if self.shared.path.is_none() {
            return Durable::ready();
        }
        let mut q = self.shared.queue.lock().expect("state file");
        if q.replace.is_some() {
            self.shared.storage.write_coalesced(self.shared.store);
        }
        q.replace = Some(Box::new(encode));
        q.appends.clear();
        self.enqueue(q)
    }

    /// Queues `line` (complete lines, `\n`-terminated) to be appended to
    /// the journal. Refused with [`NeedsRewrite`] while the file may lack
    /// earlier lines (a failed write, or more than
    /// [`MAX_QUEUED_APPEND_BYTES`] queued): the store then queues a full
    /// snapshot with [`Self::replace`] instead.
    pub fn append(&self, line: &[u8]) -> Result<Durable, NeedsRewrite> {
        if self.shared.path.is_none() {
            return Ok(Durable::ready());
        }
        let mut q = self.shared.queue.lock().expect("state file");
        if q.needs_rewrite && q.replace.is_none() {
            return Err(NeedsRewrite);
        }
        if q.appends.len() + line.len() > MAX_QUEUED_APPEND_BYTES {
            // The disk is not keeping up: drop the queued lines (memory
            // has them) and have the store write the journal whole.
            q.appends = Vec::new();
            q.replace = None;
            q.needs_rewrite = true;
            self.shared.storage.journal_overflow(self.shared.store);
            tracing::warn!(
                alert = "state_writer_behind",
                store = self.shared.store,
                "state writer behind: journal lines dropped for a full rewrite"
            );
            return Err(NeedsRewrite);
        }
        q.appends.extend_from_slice(line);
        Ok(self.enqueue(q))
    }

    fn enqueue(&self, mut q: std::sync::MutexGuard<'_, Queue>) -> Durable {
        q.seq += 1;
        let seq = q.seq;
        let slot = Arc::new(Slot::default());
        q.waiters.push((seq, slot.clone()));
        if q.pending_since.is_none() {
            let now = Instant::now();
            q.pending_since = Some(now);
            self.shared
                .storage
                .writer_pending(self.shared.store, self.shared.id, Some(now));
        }
        if !q.started {
            q.started = true;
            let shared = self.shared.clone();
            let spawned = std::thread::Builder::new()
                .name(format!("relay-persist-{}", self.shared.store))
                .spawn(move || run(&shared));
            if let Err(e) = spawned {
                q.started = false;
                let err = WriteError(format!("no state writer thread: {e}").into());
                let upto = q.seq;
                q.complete(upto, &Err(err.clone()));
                self.shared.storage.failed(self.shared.store, &err.0);
                return Durable(Some(slot));
            }
        }
        drop(q);
        self.shared.wake.notify_one();
        Durable(Some(slot))
    }

    /// Waits (blocking the calling thread, at most `timeout`) until
    /// everything queued so far is written. For shutdown and tests; never
    /// on an async worker. Returns whether the writer caught up.
    pub fn flush_blocking(&self, timeout: Duration) -> bool {
        self.shared.flush_until(Instant::now() + timeout)
    }

    /// Holds the writer's next batch until the guard is dropped, as a disk
    /// whose writes hang would (tests).
    #[cfg(test)]
    pub(crate) fn stall(&self) -> StallGuard<'_> {
        self.shared.queue.lock().expect("state file").stalled = true;
        StallGuard(self)
    }
}

#[cfg(test)]
impl StateFile {
    /// Lifts a stall from any thread (a test's watchdog).
    pub(crate) fn unstall(&self) {
        self.shared.queue.lock().expect("state file").stalled = false;
        self.shared.wake.notify_all();
    }
}

/// Lifts [`StateFile::stall`] when dropped.
#[cfg(test)]
pub(crate) struct StallGuard<'a>(&'a StateFile);

#[cfg(test)]
impl Drop for StallGuard<'_> {
    fn drop(&mut self) {
        self.0.unstall();
    }
}

impl Drop for StateFile {
    /// Lets the writer finish what is queued, waiting for it at most
    /// [`DROP_FLUSH`] (a store is dropped at shutdown, when its last
    /// changes must reach the disk), then stops it.
    fn drop(&mut self) {
        if self.shared.path.is_some() && !self.flush_blocking(DROP_FLUSH) {
            tracing::warn!(
                alert = "state_writer_behind",
                store = self.shared.store,
                "state writer still busy at shutdown; its last changes may be lost"
            );
        }
        self.shared.queue.lock().expect("state file").closed = true;
        self.shared.wake.notify_all();
    }
}

/// The writer thread.
fn run(shared: &Shared) {
    let path = shared.path.clone().expect("a persisted state file");
    loop {
        let (replace, appends, upto) = {
            let mut q = shared.queue.lock().expect("state file");
            while !q.has_work() {
                if q.closed {
                    let upto = q.seq;
                    q.complete(upto, &Err(WriteError("state writer stopped".into())));
                    return;
                }
                q = shared.wake.wait(q).expect("state file");
            }
            q.busy = true;
            (q.replace.take(), std::mem::take(&mut q.appends), q.seq)
        };
        #[cfg(test)]
        {
            let mut q = shared.queue.lock().expect("state file");
            while q.stalled {
                q = shared.wake.wait(q).expect("state file");
            }
        }
        let started = Instant::now();
        let wrote_snapshot = replace.is_some();
        let mut result: io::Result<()> = Ok(());
        if let Some(encode) = replace {
            result = write_snapshot(&path, encode);
        }
        if result.is_ok() && !appends.is_empty() {
            result = append_lines(&path, &appends);
        }
        let took = started.elapsed();
        shared.storage.record(shared.store, &result);
        shared.storage.write_took(shared.store, took);
        if took >= SLOW_WRITE {
            tracing::warn!(
                alert = "state_write_slow",
                store = shared.store,
                secs = took.as_secs_f64(),
                "state write slow"
            );
        }
        let result = result.map_err(|e| WriteError(e.to_string().into()));
        let mut q = shared.queue.lock().expect("state file");
        q.busy = false;
        q.last_ok = result.is_ok();
        match &result {
            Ok(()) if wrote_snapshot => q.needs_rewrite = false,
            Ok(()) => {}
            Err(e) => {
                // A journal may now lack lines memory has. Lines queued
                // since would land after the gap: drop them (unless a
                // snapshot, which carries them, is already queued) and
                // have the store write the journal whole. (A snapshot-only
                // file never has lines queued.)
                if q.replace.is_none() {
                    q.appends.clear();
                    q.needs_rewrite = true;
                    let seq = q.seq;
                    q.complete(seq, &Err(e.clone()));
                }
            }
        }
        q.complete(upto, &result);
        if q.has_work() {
            q.pending_since = Some(Instant::now());
        } else {
            q.pending_since = None;
        }
        shared
            .storage
            .writer_pending(shared.store, shared.id, q.pending_since);
        drop(q);
        shared.idle.notify_all();
    }
}

fn write_snapshot(path: &std::path::Path, encode: Encode) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::assertion::write_private_with(path, encode)
}

/// Appends `lines` and `fdatasync`s them; on a failure the file is cut
/// back to where it was, so no torn line stays.
fn append_lines(path: &std::path::Path, lines: &[u8]) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    let before = file.metadata()?.len();
    let written = file.write_all(lines).and_then(|()| file.sync_data());
    if written.is_err() {
        let _ = file.set_len(before);
    }
    written
}

/// The entries of a keyed map one change touched, with their values before
/// and after it: undoes the change if its write fails, without touching
/// what other changes did since.
#[derive(Debug)]
pub struct MapUndo<K, V>(Vec<(K, Option<V>, Option<V>)>);

impl<K: Ord + Clone, V: PartialEq + Clone> MapUndo<K, V> {
    /// The entries that differ between `before` and `after`.
    pub fn diff(before: &BTreeMap<K, V>, after: &BTreeMap<K, V>) -> Self {
        let mut out = Vec::new();
        for (k, v) in before {
            match after.get(k) {
                Some(a) if a == v => {}
                a => out.push((k.clone(), Some(v.clone()), a.cloned())),
            }
        }
        for (k, a) in after {
            if !before.contains_key(k) {
                out.push((k.clone(), None, Some(a.clone())));
            }
        }
        Self(out)
    }

    /// Nothing changed.
    pub fn none() -> Self {
        Self(Vec::new())
    }

    /// One entry going from `before` to `after`.
    pub fn one(key: K, before: Option<V>, after: Option<V>) -> Self {
        Self(vec![(key, before, after)])
    }

    /// Whether the change touched nothing.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Puts back the old value of each entry that still holds what this
    /// change wrote. Returns whether anything was put back.
    pub fn undo(self, current: &mut BTreeMap<K, V>) -> bool {
        let mut undone = false;
        for (k, before, after) in self.0 {
            if current.get(&k) != after.as_ref() {
                continue;
            }
            undone = true;
            match before {
                Some(v) => current.insert(k, v),
                None => current.remove(&k),
            };
        }
        undone
    }
}

/// A change to a keyed store made in memory whose write is queued: await
/// [`Pending::settle`] once the store's lock is released.
#[must_use = "settle it: a failed write must undo the change"]
pub struct Pending<K, V> {
    durable: Durable,
    undo: MapUndo<K, V>,
}

impl<K: Ord + Clone, V: PartialEq + Clone> Pending<K, V> {
    /// Replaces `current` with `next` and queues the write `write` makes
    /// of it. Call under the store's lock.
    pub fn swap(
        current: &mut BTreeMap<K, V>,
        next: BTreeMap<K, V>,
        write: impl FnOnce(&BTreeMap<K, V>) -> Durable,
    ) -> Self {
        let undo = MapUndo::diff(current, &next);
        *current = next;
        Self {
            durable: write(current),
            undo,
        }
    }

    /// Waits for the write (holding no lock, blocking no worker). On a
    /// failure the change is undone in `map` (where another change has not
    /// replaced it since) and the error returned. The file is queued again
    /// as memory now holds it only if a later write of `file` succeeded (it
    /// may carry the undone change); otherwise the disk still has the old
    /// content and the next write brings it up to date.
    pub async fn settle(
        self,
        map: &Mutex<BTreeMap<K, V>>,
        file: &StateFile,
        write: impl FnOnce(&BTreeMap<K, V>) -> Durable,
    ) -> Result<(), WriteError> {
        let err = match self.durable.await {
            Ok(()) => return Ok(()),
            Err(e) => e,
        };
        let mut current = map.lock().unwrap_or_else(|p| p.into_inner());
        if self.undo.undo(&mut current) && file.last_ok() {
            write(&current).detach();
        }
        Err(err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> Arc<Storage> {
        Arc::new(Storage::default())
    }

    fn json(v: &'static str) -> impl FnOnce(&mut dyn Write) -> io::Result<()> + Send {
        move |w| w.write_all(v.as_bytes())
    }

    #[test]
    fn snapshots_are_written_atomically_and_coalesced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.json");
        let st = storage();
        let f = StateFile::new(Some(path.clone()), "directory", st.clone());
        let stall = f.stall();
        // The first batch is taken and held; the rest queue behind it.
        let first = f.replace(json("1"));
        std::thread::sleep(Duration::from_millis(50));
        let second = f.replace(json("2"));
        let third = f.replace(json("3"));
        assert!(!path.exists(), "nothing written while the disk is stalled");
        drop(stall);
        first.wait_blocking().unwrap();
        second.wait_blocking().unwrap();
        third.wait_blocking().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "3");
        assert!(f.flush_blocking(Duration::from_secs(5)));
        assert!(st
            .metrics()
            .contains("cua_relay_state_writes_coalesced_total{store=\"directory\"} 1"));
    }

    #[test]
    fn journal_lines_follow_the_snapshot_they_were_queued_after() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.jsonl");
        let f = StateFile::new(Some(path.clone()), "audit", storage());
        f.append(b"a\n").unwrap().wait_blocking().unwrap();
        f.append(b"b\n").unwrap().wait_blocking().unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "a\nb\n");
        let stall = f.stall();
        let held = f.append(b"c\n").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let lost = f.append(b"d\n").unwrap();
        // A snapshot supersedes the line queued before it ...
        let snap = f.replace(json("A\nB\nC\nD\n"));
        // ... and lines queued after it follow it.
        let after = f.append(b"e\n").unwrap();
        drop(stall);
        for d in [held, lost, snap, after] {
            d.wait_blocking().unwrap();
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "A\nB\nC\nD\ne\n");
    }

    #[test]
    fn a_writer_too_far_behind_asks_for_a_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("j.jsonl");
        let st = storage();
        let f = StateFile::new(Some(path.clone()), "audit", st.clone());
        let stall = f.stall();
        let first = f.append(b"x\n").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let line = vec![b'y'; 1 << 20];
        let mut refused = false;
        for _ in 0..(MAX_QUEUED_APPEND_BYTES >> 20) + 2 {
            if f.append(&line).is_err() {
                refused = true;
                break;
            }
        }
        assert!(refused, "the queue is bounded");
        assert_eq!(f.append(b"z\n").err(), Some(NeedsRewrite));
        let snap = f.replace(json("whole\n"));
        assert!(
            f.append(b"next\n").is_ok(),
            "lines queue again behind the snapshot"
        );
        drop(stall);
        first.wait_blocking().unwrap();
        snap.wait_blocking().unwrap();
        assert!(f.flush_blocking(Duration::from_secs(5)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "whole\nnext\n");
        assert!(st
            .metrics()
            .contains("cua_relay_state_journal_overflows_total{store=\"audit\"} 1"));
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_append_is_reported_and_leaves_no_gap() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let path = state.join("j.jsonl");
        let st = storage();
        let f = StateFile::new(Some(path.clone()), "sessions", st.clone());
        f.append(b"a\n").unwrap().wait_blocking().unwrap();
        // Read-only file: the next append fails.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        let failed = f.append(b"b\n").unwrap().wait_blocking();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        if failed.is_err() {
            assert!(!f.last_ok());
            assert!(!st.store_ok("sessions"));
            assert_eq!(f.append(b"c\n").err(), Some(NeedsRewrite));
            f.replace(json("a\nb\nc\n")).wait_blocking().unwrap();
            assert!(f.last_ok());
        }
        f.append(b"d\n").unwrap().wait_blocking().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text == "a\nb\nc\nd\n" || text == "a\nb\nd\n", "{text}");
    }

    #[tokio::test]
    async fn durable_is_awaited_without_blocking_and_in_memory_is_ready() {
        assert!(StateFile::in_memory().replace(json("x")).await.is_ok());
        let dir = tempfile::tempdir().unwrap();
        let f = StateFile::new(Some(dir.path().join("f")), "grants", storage());
        f.replace(json("x")).await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("f")).unwrap(), "x");
    }

    #[test]
    fn map_undo_restores_only_what_still_holds_the_change() {
        let before: BTreeMap<&str, u32> = [("a", 1), ("b", 2)].into();
        let mut after = before.clone();
        after.insert("a", 10);
        after.remove("b");
        after.insert("c", 3);
        let undo = MapUndo::diff(&before, &after);
        let mut current = after.clone();
        current.insert("c", 30); // another change since
        assert!(undo.undo(&mut current));
        assert_eq!(current, [("a", 1), ("b", 2), ("c", 30)].into());
    }
}
