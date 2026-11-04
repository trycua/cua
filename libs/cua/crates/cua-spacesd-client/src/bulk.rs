//! The client end of the bulk lane: when this process runs on the machine
//! hosting a Linux VM, file content moves through a disk the two share
//! instead of the VM's user-mode network (see `cua_proto::bulk` for the
//! layout and `fs.rs` for how transfers use it). Unix only.
//!
//! A lane is only used after it is proven: the file's first block must hold
//! the identity the guest advertises in its capabilities, the file must be
//! the caller's own and private, and the guest must have a lane at all. Any
//! doubt, and a transfer takes the network path as before. Integrity is the
//! transfer's own SHA-256, checked against the guest's.

use std::{
    fs::File,
    path::Path,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use cua_proto::bulk::{HEADER_BYTES, Header};
use tokio::sync::Notify;

/// What the process shares for one bulk file: its descriptor, which slots
/// its transfers hold, and the machine-wide lock (a transfer from another
/// process waits no longer than it takes to fall back to the network).
struct Shared {
    file: File,
    lock: File,
    busy: Vec<AtomicBool>,
    /// Slots in use; the machine-wide lock is held while it is non-zero.
    active: AtomicUsize,
    gate: Mutex<()>,
    notify: Notify,
}

type Registry = Mutex<Vec<(std::path::PathBuf, Weak<Shared>)>>;

fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Default::default)
}

/// A proven lane: the header the guest wrote and the shared file.
#[derive(Clone)]
pub(crate) struct Lane {
    pub(crate) header: Header,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Lane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Lane({} slots)", self.header.slots)
    }
}

/// Whether the lane is switched off in this process (`CUA_BULK_LANE=off`).
pub(crate) fn disabled() -> bool {
    matches!(
        std::env::var("CUA_BULK_LANE").ok().as_deref(),
        Some("off" | "0" | "false" | "no")
    )
}

impl Lane {
    /// Opens `path` as the lane whose guest advertised `id`; `Ok(None)`
    /// when it is not (a missing file, another VM's file, a file that is
    /// not ours).
    pub(crate) fn open(
        path: &Path,
        id: &[u8],
        slots: u32,
        slot_bytes: u64,
        data_offset: u64,
    ) -> std::io::Result<Option<Lane>> {
        use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
        // A private regular file of ours (not a link to something else).
        let meta = std::fs::symlink_metadata(path)?;
        let key = std::fs::canonicalize(path)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode_private()
            .open(lock_path(&key))?;
        // The lock file is ours: it is created with our identity.
        let me = lock.metadata()?.uid();
        if !meta.file_type().is_file() || meta.uid() != me || meta.permissions().mode() & 0o077 != 0
        {
            return Ok(None);
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        let opened = file.metadata()?;
        if (opened.dev(), opened.ino()) != (meta.dev(), meta.ino()) {
            return Ok(None);
        }
        let mut block = vec![0u8; HEADER_BYTES];
        file.read_exact_at(&mut block, 0)?;
        let Some(header) = Header::decode(&block) else {
            return Ok(None);
        };
        if header.id.as_slice() != id
            || header.slots != slots
            || header.slot_bytes != slot_bytes
            || header.data_offset != data_offset
            || file.metadata()?.len() < header.disk_bytes()
        {
            return Ok(None);
        }
        let mut reg = registry().lock().expect("bulk registry");
        reg.retain(|(_, w)| w.strong_count() > 0);
        let shared = match reg
            .iter()
            .find(|(p, _)| *p == key)
            .and_then(|(_, w)| w.upgrade())
        {
            Some(s) => s,
            None => {
                let s = Arc::new(Shared {
                    file,
                    lock,
                    busy: (0..header.slots).map(|_| AtomicBool::new(false)).collect(),
                    active: AtomicUsize::new(0),
                    gate: Mutex::new(()),
                    notify: Notify::new(),
                });
                reg.push((key, Arc::downgrade(&s)));
                s
            }
        };
        Ok(Some(Lane { header, shared }))
    }

    /// Takes a free slot, waiting a short while for one; `None` when the
    /// lane is busy (a transfer of another process holds it, or all slots
    /// are in use for too long): the caller uses the network.
    pub(crate) async fn acquire(&self) -> Option<Slot> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            let notified = self.shared.notify.notified();
            if let Some(slot) = self.try_acquire() {
                return Some(slot);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return None;
            }
        }
    }

    fn try_acquire(&self) -> Option<Slot> {
        let s = &self.shared;
        let _g = s.gate.lock().expect("bulk gate");
        let idx = s.busy.iter().position(|b| {
            b.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        })?;
        if s.active.fetch_add(1, Ordering::AcqRel) == 0 && s.lock.try_lock().is_err() {
            // Another process is mid-transfer on this disk.
            s.active.fetch_sub(1, Ordering::AcqRel);
            s.busy[idx].store(false, Ordering::Release);
            return None;
        }
        Some(Slot {
            shared: self.shared.clone(),
            idx: idx as u32,
            offset: self.header.slot_offset(idx as u32),
            bytes: self.header.slot_bytes,
        })
    }
}

fn lock_path(p: &Path) -> std::path::PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".lock");
    s.into()
}

/// A held slot: its bytes are this transfer's alone until it is dropped.
pub(crate) struct Slot {
    shared: Arc<Shared>,
    /// Slot number.
    pub(crate) idx: u32,
    offset: u64,
    bytes: u64,
}

impl Slot {
    /// The number the protocol's `bulk_slot` carries.
    pub(crate) fn wire(&self) -> u32 {
        self.idx + 1
    }

    /// Reads `len` bytes of the slot into a new buffer.
    pub(crate) fn read(&self, len: usize) -> std::io::Result<Vec<u8>> {
        use std::os::unix::fs::FileExt;
        assert!(len as u64 <= self.bytes);
        let mut buf = vec![0u8; len];
        self.shared.file.read_exact_at(&mut buf, self.offset)?;
        Ok(buf)
    }

    /// Writes `data` at `at` bytes into the slot.
    pub(crate) fn write(&self, at: u64, data: &[u8]) -> std::io::Result<()> {
        use std::os::unix::fs::FileExt;
        assert!(at + data.len() as u64 <= self.bytes);
        self.shared.file.write_all_at(data, self.offset + at)
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let s = &self.shared;
        {
            let _g = s.gate.lock().expect("bulk gate");
            s.busy[self.idx as usize].store(false, Ordering::Release);
            if s.active.fetch_sub(1, Ordering::AcqRel) == 1 {
                let _ = s.lock.unlock();
            }
        }
        s.notify.notify_waiters();
    }
}

trait PrivateMode {
    fn mode_private(&mut self) -> &mut Self;
}

impl PrivateMode for std::fs::OpenOptions {
    fn mode_private(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.mode(0o600)
    }
}

// ---- transfers --------------------------------------------------------

use crate::{
    client::SpacesdClient,
    error::{Error, Result},
    fs::{DownloadOptions, DownloadSink, UploadOptions, UploadSource},
};
use bytes::Bytes;
use cua_proto::env::v1::{self as pb, read_file_response::Message as ReadMsg};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;

/// A window being read: its bytes and the file's size.
type Window = tokio::task::JoinHandle<Result<(Vec<u8>, u64)>>;

/// Windows a transfer keeps in flight (bounded by the lane's slots).
const DEPTH: usize = 3;

impl SpacesdClient {
    /// Looks for a bulk lane to the guest at `path`: the file the machine
    /// hosting this guest shares with it (a Linux VM's `bulk.img`). Returns
    /// whether the lane is proven and now carries this client's large
    /// transfers. Calling it for a guest that is elsewhere, has no lane, or
    /// whose file is another VM's changes nothing: transfers use the network.
    pub async fn attach_bulk(&self, path: impl AsRef<Path>) -> bool {
        if disabled() {
            return false;
        }
        // Fresh capabilities: the guest (re)writes the disk's header when it
        // answers, which a restored VM's new file needs.
        let Ok(caps) = self.refresh_capabilities().await else {
            return false;
        };
        let Some(b) = caps.bulk else {
            return false;
        };
        let path = path.as_ref().to_path_buf();
        let opened = tokio::task::spawn_blocking(move || {
            Lane::open(&path, &b.id, b.slots, b.slot_bytes, b.data_offset)
        })
        .await;
        match opened {
            Ok(Ok(Some(lane))) => {
                *self.inner.bulk.write().expect("bulk lane") = Some(lane);
                true
            }
            Ok(Ok(None)) => false,
            Ok(Err(e)) => {
                tracing::debug!(error = %e, "bulk lane not usable");
                false
            }
            Err(_) => false,
        }
    }

    /// Whether large transfers of this client use a bulk lane.
    pub fn bulk_attached(&self) -> bool {
        self.inner.bulk.read().expect("bulk lane").is_some()
    }

    pub(crate) fn bulk_lane(&self) -> Option<Lane> {
        self.inner.bulk.read().expect("bulk lane").clone()
    }

    fn drop_bulk(&self) {
        *self.inner.bulk.write().expect("bulk lane") = None;
    }

    /// Reads one window of `path` through a slot: the bytes, the file's size
    /// and whether the guest's digest matched.
    async fn bulk_window(
        &self,
        lane: &Lane,
        path: &str,
        offset: u64,
        len: u64,
    ) -> Result<(Vec<u8>, u64)> {
        let slot = lane
            .acquire()
            .await
            .ok_or_else(|| Error::Protocol("bulk lane busy".into()))?;
        let req = pb::ReadFileRequest {
            path: path.to_string(),
            offset,
            length: len,
            chunk_size: 0,
            compute_sha256: true,
            bulk_slot: slot.wire(),
            // The bulk lane reads as cua-spacesd's own user, with the
            // default metadata namespace.
            xattr_namespace: String::new(),
            run_as: String::new(),
        };
        let mut stream = self.filesystem().read_file(req).await?.into_inner();
        let (mut size, mut end) = (0u64, None);
        while let Some(msg) = stream.message().await? {
            match msg.message {
                Some(ReadMsg::Entry(e)) => size = e.size,
                Some(ReadMsg::End(e)) => end = Some(e),
                Some(ReadMsg::Chunk(_)) => {
                    return Err(Error::Protocol("chunk in a bulk read".into()));
                }
                None => {}
            }
        }
        let end = end.ok_or_else(|| Error::Protocol("bulk read without an end".into()))?;
        let n = end.bytes_read as usize;
        let data = tokio::task::spawn_blocking(move || slot.read(n))
            .await
            .map_err(|e| Error::Protocol(e.to_string()))??;
        let expected = end.sha256;
        let (data, got) = tokio::task::spawn_blocking(move || {
            let d = hex::encode(Sha256::digest(&data));
            (data, d)
        })
        .await
        .map_err(|e| Error::Protocol(e.to_string()))?;
        if !expected.eq_ignore_ascii_case(&got) {
            return Err(Error::ChecksumMismatch {
                expected,
                actual: got,
            });
        }
        Ok((data, size))
    }

    /// Downloads through the lane from `options.offset`, feeding `sink` and
    /// `total`. Returns `Ok(true)` when the whole range arrived. On `Err`
    /// (or `Ok(false)`) the first `received` bytes were delivered and the
    /// caller continues over the network from there.
    // The caller's running state (hash, byte count, size hint) is passed
    // piece by piece: the network path below continues from it on failure.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn bulk_pull<S: DownloadSink>(
        &self,
        lane: &Lane,
        path: &str,
        options: &DownloadOptions,
        sink: &mut S,
        total: &mut Sha256,
        received: &mut u64,
        sized: &mut bool,
    ) -> Result<bool> {
        let window = lane.header.slot_bytes;
        let end_limit = (options.length > 0).then(|| options.offset + options.length);
        let mut size_end: Option<u64> = None;
        let mut next = options.offset + *received;
        let mut inflight: VecDeque<Window> = VecDeque::new();
        let mut asked = 0u64;
        let result = loop {
            // The file's size is known after the first window: until then
            // one request is in flight.
            while inflight.len() < DEPTH.min(lane.header.slots as usize)
                && (size_end.is_some() || inflight.is_empty())
            {
                let stop = match (end_limit, size_end) {
                    (Some(a), Some(b)) => a.min(b),
                    (Some(a), None) => a,
                    (None, Some(b)) => b,
                    (None, None) => u64::MAX,
                };
                if next >= stop {
                    break;
                }
                let len = window.min(stop - next);
                let (client, lane, path) = (self.clone(), lane.clone(), path.to_string());
                let off = next;
                inflight.push_back(tokio::spawn(async move {
                    client.bulk_window(&lane, &path, off, len).await
                }));
                next += len;
                asked += len;
            }
            let Some(handle) = inflight.pop_front() else {
                break Ok(true);
            };
            let (data, size) = match handle.await {
                Ok(Ok(w)) => w,
                Ok(Err(e)) => break Err(e),
                Err(e) => break Err(Error::Protocol(e.to_string())),
            };
            if !*sized {
                *sized = true;
                let rest = size.saturating_sub(options.offset);
                sink.size_hint(end_limit.map_or(rest, |_| options.length.min(rest)));
            }
            size_end = Some(size);
            let short = (data.len() as u64) < window.min(asked);
            total.update(&data);
            *received += data.len() as u64;
            if let Some(p) = &options.progress {
                p.store(*received, Ordering::Relaxed);
            }
            if let Err(e) = sink.write(Bytes::from(data)).await {
                break Err(e);
            }
            // A window shorter than asked is the end of the file.
            if short && inflight.is_empty() {
                break Ok(true);
            }
        };
        for h in inflight {
            h.abort();
        }
        // The delivered prefix is exact only for in-order windows; a failed
        // window ends the pull with everything before it written.
        if let Err(e) = &result {
            tracing::debug!(error = %e, received = *received, "bulk download stopped; continuing over the network");
            if is_no_lane(e) {
                self.drop_bulk();
            }
        }
        result
    }

    /// Uploads through the lane. `Ok(None)`: the lane could not carry it
    /// (nothing was written to the destination); the caller uses the
    /// network.
    pub(crate) async fn bulk_push(
        &self,
        lane: &Lane,
        path: &str,
        source: &UploadSource,
        size: u64,
        sha: &str,
        options: &UploadOptions,
    ) -> Result<Option<crate::fs::UploadResult>> {
        let begin = pb::BeginUploadRequest {
            header: Some(Self::upload_header(path, size, sha, options)),
            upload_id: String::new(),
            ttl: options.ttl.map(crate::process::duration_pb),
        };
        let started = self.filesystem().begin_upload(begin).await?.into_inner();
        let upload_id = started.upload_id;
        let window = lane.header.slot_bytes;
        let run = async {
            // Window i+1 is copied into its slot while the guest takes window i.
            let fill = |offset: u64| {
                let (lane, source) = (lane.clone(), source.clone());
                let len = window.min(size - offset) as usize;
                tokio::spawn(async move {
                    let slot = lane
                        .acquire()
                        .await
                        .ok_or_else(|| Error::Protocol("bulk lane busy".into()))?;
                    let slot = tokio::task::spawn_blocking(move || -> Result<Slot> {
                        source.copy_to_slot(offset, len, &slot)?;
                        Ok(slot)
                    })
                    .await
                    .map_err(|e| Error::Protocol(e.to_string()))??;
                    Ok::<_, Error>((slot, len as u64))
                })
            };
            let mut offset = 0u64;
            let mut pending: VecDeque<tokio::task::JoinHandle<Result<(Slot, u64)>>> =
                VecDeque::new();
            let mut next = 0u64;
            while offset < size {
                while pending.len() < 2.min(lane.header.slots as usize) && next < size {
                    pending.push_back(fill(next));
                    next += window.min(size - next);
                }
                let (slot, len) = pending
                    .pop_front()
                    .expect("a window is pending")
                    .await
                    .map_err(|e| Error::Protocol(e.to_string()))??;
                let req = pb::UploadChunkRequest {
                    upload_id: upload_id.clone(),
                    offset,
                    data: Vec::new(),
                    bulk_slot: slot.wire(),
                    bulk_len: len,
                };
                let resp = self.filesystem().upload_chunk(req).await?.into_inner();
                drop(slot);
                offset += len;
                if resp.received_bytes != offset {
                    return Err(Error::Protocol(format!(
                        "the guest holds {} bytes, expected {offset}",
                        resp.received_bytes
                    )));
                }
                if let Some(p) = &options.progress {
                    p.store(offset, Ordering::Relaxed);
                }
            }
            let committed = self
                .filesystem()
                .commit_upload(pb::CommitUploadRequest {
                    upload_id: upload_id.clone(),
                    sha256: sha.to_string(),
                })
                .await?
                .into_inner();
            Ok(committed)
        };
        match run.await {
            Ok(c) => Ok(Some(crate::fs::UploadResult {
                entry: c.entry,
                sha256: c.sha256,
                size,
                resumable: true,
                resumes: 0,
            })),
            Err(e) => {
                tracing::debug!(error = %e, "bulk upload failed; using the network");
                let _ = self
                    .filesystem()
                    .abort_upload(pb::AbortUploadRequest {
                        upload_id: upload_id.clone(),
                    })
                    .await;
                if let Some(p) = &options.progress {
                    p.store(0, Ordering::Relaxed);
                }
                if is_no_lane(&e) {
                    self.drop_bulk();
                }
                Ok(None)
            }
        }
    }
}

impl UploadSource {
    /// Copies `len` bytes from `offset` into `slot`.
    fn copy_to_slot(&self, offset: u64, len: usize, slot: &Slot) -> Result<()> {
        use std::os::unix::fs::FileExt;
        match self {
            UploadSource::Bytes(b) => {
                let start = (offset as usize).min(b.len());
                let end = (start + len).min(b.len());
                slot.write(0, &b[start..end])?;
                if end - start != len {
                    return Err(Error::Protocol("source ended early".into()));
                }
            }
            UploadSource::File(p) => {
                let f = std::fs::File::open(p)?;
                let mut buf = vec![0u8; len.min(4 << 20)];
                let mut done = 0usize;
                while done < len {
                    let want = buf.len().min(len - done);
                    let n = f.read_at(&mut buf[..want], offset + done as u64)?;
                    if n == 0 {
                        return Err(Error::Protocol("source ended early".into()));
                    }
                    slot.write(done as u64, &buf[..n])?;
                    done += n;
                }
            }
        }
        Ok(())
    }
}

/// The guest answered that it has no bulk lane (any more).
fn is_no_lane(e: &Error) -> bool {
    matches!(e, Error::Rpc(d) if d.message.contains("no bulk lane"))
}
