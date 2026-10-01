// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The drive as a filesystem: inodes, directories, streaming reads and
//! spooled writes, shared by every mount (NFS on macOS, FUSE on Linux, and
//! the FSKit module once it is signed).
//!
//! Every operation runs as one [`Session`], so the access rules, the audit
//! log and the secret scanner apply exactly as they do to tool calls.
//!
//! - **Reads** pin the file's current version when first read and stream
//!   ranges of it through the [`Streamer`] (block cache and read-ahead on a
//!   remote backend). A concurrent write elsewhere never mixes contents;
//!   the next read after the change is seen reads the new version.
//! - **Writes** go to a local spool file (`<state>/spool`). The spool is
//!   uploaded (multipart on S3) when the file is closed or flushed (FUSE),
//!   or after [`UPLOAD_IDLE`] without writes (NFS has no close). Until it
//!   lands, reads and listings see the spool.
//! - **Conflicts**: if the file changed in the store since the spool was
//!   based on it, the store's version is copied to a visible conflict copy
//!   first, then the local write lands as the current version (last writer
//!   wins). Both stay in the file's history.
//! - **Listings** are cached per folder and invalidated by local changes
//!   and by the change feed, with [`DIR_TTL`] as a safety net.
//! - Finder metadata (`.DS_Store`, `._*`) stays on this machine
//!   (`<state>/local`) and never uploads.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::backend::{Condition, ObjectMeta};
use crate::cache::BlockCache;
use crate::drive::{Change, ChangeSink};
use crate::feed::{Conflict, Feed, FeedEntry, RemoteListener, conflict_path};
use crate::stream::Streamer;
use crate::{Context, Drive, Error, Mode, Result, Session, now_ms};

/// The root's inode.
pub const ROOT: u64 = 1;
/// How long a folder listing is trusted without an invalidation.
pub const DIR_TTL: Duration = Duration::from_secs(15);
/// How long a pinned version is trusted without an invalidation.
pub const PIN_TTL: Duration = Duration::from_secs(30);
/// Quiet time after the last write before a spool uploads (mounts without
/// a close signal).
pub const UPLOAD_IDLE: Duration = Duration::from_millis(1500);

/// One file or folder's attributes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Attr {
    pub ino: u64,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ms: u64,
    pub writable: bool,
}

/// The operations a mount adapter needs, over a local [`Vfs`] or a remote
/// one (a Space's guest reaching its host through
/// [`crate::remote::RemoteFs`]).
#[async_trait::async_trait]
pub trait FsOps: Send + Sync {
    async fn getattr(&self, ino: u64) -> Result<Attr>;
    async fn lookup(&self, parent: u64, name: &str) -> Result<Attr>;
    async fn readdir(&self, ino: u64) -> Result<Vec<(String, Attr)>>;
    async fn read(&self, ino: u64, offset: u64, len: u64) -> Result<(Vec<u8>, bool)>;
    async fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<Attr>;
    async fn create(&self, parent: u64, name: &str) -> Result<Attr>;
    async fn truncate(&self, ino: u64, size: u64) -> Result<Attr>;
    async fn mkdir(&self, parent: u64, name: &str) -> Result<Attr>;
    async fn unlink(&self, parent: u64, name: &str) -> Result<()>;
    async fn rmdir(&self, parent: u64, name: &str) -> Result<()>;
    async fn rename(&self, from_dir: u64, from: &str, to_dir: u64, to: &str) -> Result<()>;
    /// Uploads the file behind `ino` now (a close or an fsync).
    async fn flush_ino(&self, ino: u64) -> Result<()>;
    /// Uploads everything pending (unmount).
    async fn flush_all(&self) -> Result<()>;
    /// Hands on writes still held on this side for `ino` (a close), and
    /// reports a write that failed after it was acknowledged. Nothing to do
    /// for a local [`Vfs`]; see [`crate::remote::WriteBack`].
    async fn sync_writes(&self, _ino: u64) -> Result<()> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl FsOps for Vfs {
    async fn getattr(&self, ino: u64) -> Result<Attr> {
        Vfs::getattr(self, ino).await
    }
    async fn lookup(&self, parent: u64, name: &str) -> Result<Attr> {
        Vfs::lookup(self, parent, name).await
    }
    async fn readdir(&self, ino: u64) -> Result<Vec<(String, Attr)>> {
        Vfs::readdir(self, ino).await
    }
    async fn read(&self, ino: u64, offset: u64, len: u64) -> Result<(Vec<u8>, bool)> {
        Vfs::read(self, ino, offset, len).await
    }
    async fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<Attr> {
        Vfs::write(self, ino, offset, data).await
    }
    async fn create(&self, parent: u64, name: &str) -> Result<Attr> {
        Vfs::create(self, parent, name).await
    }
    async fn truncate(&self, ino: u64, size: u64) -> Result<Attr> {
        Vfs::truncate(self, ino, size).await
    }
    async fn mkdir(&self, parent: u64, name: &str) -> Result<Attr> {
        Vfs::mkdir(self, parent, name).await
    }
    async fn unlink(&self, parent: u64, name: &str) -> Result<()> {
        Vfs::unlink(self, parent, name).await
    }
    async fn rmdir(&self, parent: u64, name: &str) -> Result<()> {
        Vfs::rmdir(self, parent, name).await
    }
    async fn rename(&self, from_dir: u64, from: &str, to_dir: u64, to: &str) -> Result<()> {
        Vfs::rename(self, from_dir, from, to_dir, to).await
    }
    async fn flush_ino(&self, ino: u64) -> Result<()> {
        let key = self.key_of(ino)?;
        self.flush(&key).await
    }
    async fn flush_all(&self) -> Result<()> {
        Vfs::flush_all(self).await
    }
}

/// The drive error for an errno (the inverse of [`errno`], for errors that
/// crossed a wire).
pub fn error_from_errno(code: i32, msg: String) -> Error {
    match code {
        libc_errno::ENOENT => Error::NotFound(msg),
        libc_errno::EACCES => Error::Forbidden(msg),
        libc_errno::EINVAL | libc_errno::ENOTDIR | libc_errno::EISDIR => Error::Invalid(msg),
        libc_errno::ENOTEMPTY => Error::Precondition(format!("{msg} (not empty)")),
        libc_errno::EEXIST => Error::Precondition(msg),
        libc_errno::EBUSY => Error::LeaseHeld {
            holder: msg,
            expires_ms: 0,
        },
        _ => Error::Backend(msg),
    }
}

/// An errno for a drive error (mount adapters report these).
pub fn errno(e: &Error) -> i32 {
    match e {
        Error::NotFound(_) => libc_errno::ENOENT,
        Error::Forbidden(_) | Error::NotConfirmed(_) => libc_errno::EACCES,
        Error::SecretDetected { .. } => libc_errno::EACCES,
        Error::Invalid(_) => libc_errno::EINVAL,
        Error::Precondition(m) if m.contains("not empty") => libc_errno::ENOTEMPTY,
        Error::Precondition(_) => libc_errno::EEXIST,
        Error::LeaseHeld { .. } => libc_errno::EBUSY,
        Error::Backend(_) => libc_errno::EIO,
    }
}

/// The POSIX numbers (same on macOS and Linux for these).
pub mod libc_errno {
    pub const ENOENT: i32 = 2;
    pub const EIO: i32 = 5;
    pub const EBUSY: i32 = 16;
    pub const EEXIST: i32 = 17;
    pub const ENOTDIR: i32 = 20;
    pub const EISDIR: i32 = 21;
    pub const EINVAL: i32 = 22;
    pub const EACCES: i32 = 13;
    #[cfg(target_os = "linux")]
    pub const ENOTEMPTY: i32 = 39;
    #[cfg(not(target_os = "linux"))]
    pub const ENOTEMPTY: i32 = 66;
}

/// Finder and Spotlight bookkeeping that stays on this machine.
pub fn local_only(name: &str) -> bool {
    name == ".DS_Store"
        || name.starts_with("._")
        || matches!(
            name,
            ".localized"
                | ".hidden"
                | ".metadata_never_index"
                | ".com.apple.timemachine.donotpresent"
        )
}

#[derive(Clone, Debug)]
struct Node {
    /// `a/b.txt` for a file, `a/b/` for a folder, `""` for the root.
    key: String,
    is_dir: bool,
}

#[derive(Clone, Debug)]
struct DirSnap {
    files: BTreeMap<String, ObjectMeta>,
    dirs: BTreeMap<String, ()>,
    writable: HashMap<String, bool>,
    at: Instant,
}

#[derive(Debug)]
struct Dirty {
    spool: PathBuf,
    /// The etag the spool was based on (`None`: a new file).
    base_etag: Option<String>,
    /// Bumped by every write; an upload only clears the entry when the
    /// generation it uploaded is still current.
    generation: u64,
    last_write: Instant,
    uploading: bool,
    /// The last upload failure (kept until the next write retries).
    error: Option<String>,
    mtime_ms: u64,
}

#[derive(Default)]
struct Inodes {
    by_key: HashMap<String, u64>,
    by_ino: HashMap<u64, Node>,
    next: u64,
}

/// The filesystem view of a drive for one session.
pub struct Vfs {
    drive: Drive,
    ctx: RwLock<Context>,
    cache: Option<Arc<BlockCache>>,
    streamer: RwLock<(String, Arc<Streamer>)>,
    feed: Option<Arc<Feed>>,
    spool_dir: PathBuf,
    local_dir: PathBuf,
    inodes: Mutex<Inodes>,
    dirs: Mutex<HashMap<String, DirSnap>>,
    dir_mtime: Mutex<HashMap<String, u64>>,
    pins: Mutex<HashMap<String, (ObjectMeta, Instant)>>,
    dirty: Mutex<HashMap<String, Dirty>>,
    started_ms: u64,
}

impl std::fmt::Debug for Vfs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vfs").field("ctx", &self.context()).finish()
    }
}

struct Invalidator(std::sync::Weak<Vfs>);

impl ChangeSink for Invalidator {
    fn changed(&self, change: Change) {
        if let Some(v) = self.0.upgrade() {
            match change {
                Change::Put(m) => v.invalidate_key(&m.key.clone(), Some(m)),
                Change::Delete(k) => v.invalidate_key(&k, None),
            }
        }
    }
}

impl RemoteListener for Invalidator {
    fn remote_changed(&self, e: &FeedEntry) {
        if let Some(v) = self.0.upgrade() {
            v.invalidate_key(&e.key, None);
        }
    }
}

fn parent_of(key: &str) -> String {
    let k = key.trim_end_matches('/');
    match k.rfind('/') {
        Some(i) => k[..=i].to_string(),
        None => String::new(),
    }
}

fn name_of(key: &str) -> &str {
    let k = key.trim_end_matches('/');
    match k.rfind('/') {
        Some(i) => &k[i + 1..],
        None => k,
    }
}

fn check_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(Error::Invalid(format!("name {name:?}")));
    }
    if name.starts_with(crate::drive::INTERNAL_PREFIX) {
        return Err(Error::Invalid(format!("{name}: reserved name")));
    }
    Ok(())
}

impl Vfs {
    /// A filesystem over `drive` as `ctx`, spooling under `state`
    /// (`<state>/spool`, `<state>/local`). Registers itself for local and
    /// remote change notices.
    pub fn new(
        drive: &Drive,
        ctx: Context,
        cache: Option<Arc<BlockCache>>,
        feed: Option<Arc<Feed>>,
        state: &Path,
    ) -> Result<Arc<Vfs>> {
        let spool_dir = state.join("spool");
        let local_dir = state.join("local");
        fs::create_dir_all(&spool_dir)?;
        fs::create_dir_all(&local_dir)?;
        // Spools of a previous run that never uploaded are kept aside, not
        // silently dropped.
        if let Ok(rd) = fs::read_dir(&spool_dir) {
            let leftovers: Vec<_> = rd.flatten().collect();
            if !leftovers.is_empty() {
                let keep = state.join(format!("spool-orphaned-{}", now_ms()));
                let _ = fs::rename(&spool_dir, &keep);
                fs::create_dir_all(&spool_dir)?;
            }
        }
        let backend = drive.backend();
        let streamer = Arc::new(Streamer::new(backend.clone(), cache.clone()));
        let mut inodes = Inodes {
            next: ROOT + 1,
            ..Default::default()
        };
        inodes.by_key.insert(String::new(), ROOT);
        inodes.by_ino.insert(
            ROOT,
            Node {
                key: String::new(),
                is_dir: true,
            },
        );
        let vfs = Arc::new(Vfs {
            drive: drive.clone(),
            ctx: RwLock::new(ctx),
            cache,
            streamer: RwLock::new((backend.identity(), streamer)),
            feed: feed.clone(),
            spool_dir,
            local_dir,
            inodes: Mutex::new(inodes),
            dirs: Mutex::new(HashMap::new()),
            dir_mtime: Mutex::new(HashMap::new()),
            pins: Mutex::new(HashMap::new()),
            dirty: Mutex::new(HashMap::new()),
            started_ms: now_ms(),
        });
        let inv = Arc::new(Invalidator(Arc::downgrade(&vfs)));
        drive.add_sink(inv.clone());
        if let Some(f) = &feed {
            f.add_listener(inv);
        }
        Ok(vfs)
    }

    fn session(&self) -> Session {
        self.drive.session(self.context())
    }

    /// Who this filesystem acts as.
    pub fn context(&self) -> Context {
        self.ctx.read().unwrap().clone()
    }

    /// Acts as `ctx` from now on, without remounting: a Space's volume
    /// becomes its agent's view while a persistent agent runs there, and
    /// the Space's own view again after. Writes still spooled land first,
    /// as the principal that made them; directory listings are read again.
    pub async fn set_context(&self, ctx: Context) -> Result<()> {
        if self.context() == ctx {
            return Ok(());
        }
        self.flush_all().await?;
        *self.ctx.write().unwrap() = ctx;
        self.dirs.lock().unwrap().clear();
        self.pins.lock().unwrap().clear();
        // Every folder reads as changed, so the guest's kernel lists it again.
        let now = now_ms();
        let mut m = self.dir_mtime.lock().unwrap();
        for v in m.values_mut() {
            *v = now;
        }
        m.insert(String::new(), now);
        Ok(())
    }

    /// The streamer for the drive's current backend (rebuilt after a
    /// storage switch).
    pub fn streamer(&self) -> Arc<Streamer> {
        let backend = self.drive.backend();
        let id = backend.identity();
        {
            let s = self.streamer.read().unwrap();
            if s.0 == id {
                return s.1.clone();
            }
        }
        let st = Arc::new(Streamer::new(backend, self.cache.clone()));
        *self.streamer.write().unwrap() = (id, st.clone());
        self.dirs.lock().unwrap().clear();
        self.pins.lock().unwrap().clear();
        st
    }

    fn invalidate_key(&self, key: &str, put: Option<ObjectMeta>) {
        let parent = parent_of(key);
        let now = now_ms();
        {
            let mut dirs = self.dirs.lock().unwrap();
            dirs.remove(&parent);
            // A new folder marker (or a first file in a new folder) changes
            // every ancestor's listing too.
            let mut p = parent.clone();
            while !p.is_empty() {
                p = parent_of(&p);
                dirs.remove(&p);
            }
        }
        {
            let mut m = self.dir_mtime.lock().unwrap();
            let mut p = parent;
            loop {
                m.insert(p.clone(), now);
                if p.is_empty() {
                    break;
                }
                p = parent_of(&p);
            }
        }
        let mut pins = self.pins.lock().unwrap();
        match put {
            Some(m) if !m.version.is_empty() => {
                pins.insert(key.to_string(), (m, Instant::now()));
            }
            _ => {
                pins.remove(key);
            }
        }
    }

    fn ino_for(&self, key: &str, is_dir: bool) -> u64 {
        let mut ix = self.inodes.lock().unwrap();
        if let Some(i) = ix.by_key.get(key) {
            return *i;
        }
        let ino = ix.next;
        ix.next += 1;
        ix.by_key.insert(key.to_string(), ino);
        ix.by_ino.insert(
            ino,
            Node {
                key: key.to_string(),
                is_dir,
            },
        );
        ino
    }

    fn node(&self, ino: u64) -> Result<Node> {
        self.inodes
            .lock()
            .unwrap()
            .by_ino
            .get(&ino)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("inode {ino}")))
    }

    fn dir_node(&self, ino: u64) -> Result<Node> {
        let n = self.node(ino)?;
        if !n.is_dir {
            return Err(Error::Invalid(format!("{} is not a folder", n.key)));
        }
        Ok(n)
    }

    /// The key an inode names now.
    pub fn key_of(&self, ino: u64) -> Result<String> {
        Ok(self.node(ino)?.key)
    }

    fn local_path(&self, key: &str) -> PathBuf {
        let mut p = self.local_dir.clone();
        for c in key.split('/').filter(|c| !c.is_empty()) {
            p.push(c);
        }
        p
    }

    async fn snapshot(&self, folder: &str) -> Result<DirSnap> {
        // Touch the streamer so a backend switch clears stale listings.
        let _ = self.streamer();
        if let Some(s) = self.dirs.lock().unwrap().get(folder)
            && s.at.elapsed() < DIR_TTL
        {
            return Ok(s.clone());
        }
        let entries = self.session().ls(folder).await?;
        let mut snap = DirSnap {
            files: BTreeMap::new(),
            dirs: BTreeMap::new(),
            writable: HashMap::new(),
            at: Instant::now(),
        };
        for e in entries {
            snap.writable
                .insert(e.name.clone(), e.mode == Mode::ReadWrite);
            if e.folder {
                snap.dirs.insert(e.name, ());
            } else {
                snap.files.insert(
                    e.name.clone(),
                    ObjectMeta {
                        key: e.path,
                        size: e.size,
                        etag: e.etag,
                        version: String::new(),
                        modified_ms: e.modified_ms,
                    },
                );
            }
        }
        self.dirs
            .lock()
            .unwrap()
            .insert(folder.to_string(), snap.clone());
        Ok(snap)
    }

    fn dir_attr(&self, ino: u64, key: &str, writable: bool) -> Attr {
        Attr {
            ino,
            is_dir: true,
            size: 0,
            mtime_ms: self
                .dir_mtime
                .lock()
                .unwrap()
                .get(key)
                .copied()
                .unwrap_or(self.started_ms),
            writable,
        }
    }

    fn dirty_attr(&self, key: &str) -> Option<(u64, u64)> {
        let d = self.dirty.lock().unwrap();
        let x = d.get(key)?;
        let size = fs::metadata(&x.spool).map(|m| m.len()).unwrap_or(0);
        Some((size, x.mtime_ms))
    }

    /// Attributes of an inode.
    pub async fn getattr(&self, ino: u64) -> Result<Attr> {
        let n = self.node(ino)?;
        if n.key.is_empty() {
            return Ok(self.dir_attr(ino, "", self.context().principal == crate::Principal::User));
        }
        self.attr_of(&n.key, n.is_dir).await
    }

    async fn attr_of(&self, key: &str, is_dir: bool) -> Result<Attr> {
        let parent = parent_of(key);
        let name = name_of(key).to_string();
        if !is_dir {
            if let Some((size, mtime)) = self.dirty_attr(key) {
                return Ok(Attr {
                    ino: self.ino_for(key, false),
                    is_dir: false,
                    size,
                    mtime_ms: mtime,
                    writable: true,
                });
            }
            if local_only(&name) {
                let m = fs::metadata(self.local_path(key))
                    .map_err(|_| Error::NotFound(key.to_string()))?;
                return Ok(Attr {
                    ino: self.ino_for(key, false),
                    is_dir: false,
                    size: m.len(),
                    mtime_ms: mtime_ms(&m),
                    writable: true,
                });
            }
        }
        let snap = self.snapshot(&parent).await?;
        let writable = snap.writable.get(&name).copied().unwrap_or(false);
        if is_dir {
            if snap.dirs.contains_key(&name) {
                return Ok(self.dir_attr(self.ino_for(key, true), key, writable));
            }
        } else if let Some(m) = snap.files.get(&name) {
            return Ok(Attr {
                ino: self.ino_for(key, false),
                is_dir: false,
                size: m.size,
                mtime_ms: m.modified_ms,
                writable,
            });
        }
        Err(Error::NotFound(key.to_string()))
    }

    /// The child `name` of folder `parent`.
    pub async fn lookup(&self, parent: u64, name: &str) -> Result<Attr> {
        let p = self.dir_node(parent)?;
        if name.starts_with(crate::drive::INTERNAL_PREFIX) || name.contains('/') {
            return Err(Error::NotFound(name.to_string()));
        }
        let file_key = format!("{}{name}", p.key);
        if self.dirty.lock().unwrap().contains_key(&file_key) {
            return self.attr_of(&file_key, false).await;
        }
        if local_only(name) {
            return self.attr_of(&file_key, false).await;
        }
        let snap = self.snapshot(&p.key).await?;
        if snap.files.contains_key(name) {
            return self.attr_of(&file_key, false).await;
        }
        if snap.dirs.contains_key(name) {
            return self.attr_of(&format!("{file_key}/"), true).await;
        }
        Err(Error::NotFound(file_key))
    }

    /// Walks from the root to `key` (`a/b/` for a folder, `a/b` for a
    /// file).
    pub async fn lookup_path(&self, key: &str) -> Result<Attr> {
        let mut attr = self.getattr(ROOT).await?;
        for c in key.split('/').filter(|c| !c.is_empty()) {
            attr = self.lookup(attr.ino, c).await?;
        }
        Ok(attr)
    }

    /// A folder's children, sorted by name (deterministic for paging).
    pub async fn readdir(&self, ino: u64) -> Result<Vec<(String, Attr)>> {
        let p = self.dir_node(ino)?;
        let snap = self.snapshot(&p.key).await?;
        let mut out: BTreeMap<String, Attr> = BTreeMap::new();
        for name in snap.dirs.keys() {
            let key = format!("{}{name}/", p.key);
            let w = snap.writable.get(name).copied().unwrap_or(false);
            out.insert(
                name.clone(),
                self.dir_attr(self.ino_for(&key, true), &key, w),
            );
        }
        for (name, m) in &snap.files {
            let key = format!("{}{name}", p.key);
            let (size, mtime) = self.dirty_attr(&key).unwrap_or((m.size, m.modified_ms));
            out.insert(
                name.clone(),
                Attr {
                    ino: self.ino_for(&key, false),
                    is_dir: false,
                    size,
                    mtime_ms: mtime,
                    writable: snap.writable.get(name).copied().unwrap_or(false),
                },
            );
        }
        // Files written here and not uploaded yet.
        let pending: Vec<String> = self
            .dirty
            .lock()
            .unwrap()
            .keys()
            .filter(|k| parent_of(k) == p.key)
            .cloned()
            .collect();
        for key in pending {
            let name = name_of(&key).to_string();
            if !out.contains_key(&name)
                && let Some((size, mtime)) = self.dirty_attr(&key)
            {
                out.insert(
                    name,
                    Attr {
                        ino: self.ino_for(&key, false),
                        is_dir: false,
                        size,
                        mtime_ms: mtime,
                        writable: true,
                    },
                );
            }
        }
        // Finder metadata kept here.
        if let Ok(rd) = fs::read_dir(self.local_path(&p.key)) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if local_only(&name)
                    && let Ok(m) = e.metadata()
                    && m.is_file()
                {
                    let key = format!("{}{name}", p.key);
                    out.insert(
                        name,
                        Attr {
                            ino: self.ino_for(&key, false),
                            is_dir: false,
                            size: m.len(),
                            mtime_ms: mtime_ms(&m),
                            writable: true,
                        },
                    );
                }
            }
        }
        Ok(out.into_iter().collect())
    }

    async fn pin(&self, key: &str) -> Result<ObjectMeta> {
        if let Some((m, at)) = self.pins.lock().unwrap().get(key)
            && at.elapsed() < PIN_TTL
        {
            return Ok(m.clone());
        }
        let m = self.session().open(key, None).await?;
        self.pins
            .lock()
            .unwrap()
            .insert(key.to_string(), (m.clone(), Instant::now()));
        Ok(m)
    }

    /// Reads `len` bytes at `offset`; returns the bytes and whether the
    /// read reached the end of the file.
    pub async fn read(&self, ino: u64, offset: u64, len: u64) -> Result<(Vec<u8>, bool)> {
        let n = self.node(ino)?;
        if n.is_dir {
            return Err(Error::Invalid(format!("{} is a folder", n.key)));
        }
        let spool = self
            .dirty
            .lock()
            .unwrap()
            .get(&n.key)
            .map(|d| d.spool.clone());
        let local = local_only(name_of(&n.key)).then(|| self.local_path(&n.key));
        if let Some(path) = spool.or(local) {
            return tokio::task::spawn_blocking(move || read_file_range(&path, offset, len))
                .await
                .map_err(|e| Error::Backend(e.to_string()))?;
        }
        let m = self.pin(&n.key).await?;
        let bytes = self
            .streamer()
            .read(&n.key, &m.version, m.size, offset, len)
            .await?;
        let eof = offset + bytes.len() as u64 >= m.size;
        Ok((bytes, eof))
    }

    /// Makes sure `key` has a spool, copying the current content in unless
    /// `truncate` (the caller replaces it anyway).
    async fn ensure_dirty(&self, key: &str, truncate: bool) -> Result<()> {
        if let Some(d) = self.dirty.lock().unwrap().get_mut(key) {
            if truncate {
                File::create(&d.spool)?;
            }
            return Ok(());
        }
        let name = name_of(key).to_string();
        check_name(&name)?;
        self.session()
            .mode(key)?
            .filter(|m| *m == Mode::ReadWrite)
            .ok_or_else(|| Error::Forbidden(format!("{key} is read-only here")))?;
        let spool = self.spool_dir.join(crate::new_id());
        let existing = self.session().drive().backend().head(key).await?;
        {
            let mut f = File::create(&spool)?;
            if existing.is_some() && !truncate {
                // Copy the current content in, a chunk at a time.
                let meta = self.session().open(key, None).await?;
                let st = self.streamer();
                let mut off = 0;
                while off < meta.size {
                    let chunk = st.read(key, &meta.version, meta.size, off, 8 << 20).await?;
                    if chunk.is_empty() {
                        break;
                    }
                    f.write_all(&chunk)?;
                    off += chunk.len() as u64;
                }
            }
        }
        let mut d = self.dirty.lock().unwrap();
        // Another writer may have raced us; keep theirs.
        if d.contains_key(key) {
            let _ = fs::remove_file(&spool);
            return Ok(());
        }
        d.insert(
            key.to_string(),
            Dirty {
                spool,
                base_etag: existing.map(|m| m.etag),
                generation: 0,
                last_write: Instant::now(),
                uploading: false,
                error: None,
                mtime_ms: now_ms(),
            },
        );
        Ok(())
    }

    fn touch_dirty(&self, key: &str) {
        let size = {
            let mut d = self.dirty.lock().unwrap();
            let Some(x) = d.get_mut(key) else { return };
            x.generation += 1;
            x.last_write = Instant::now();
            x.mtime_ms = now_ms();
            x.error = None;
            fs::metadata(&x.spool).map(|m| m.len()).unwrap_or(0)
        };
        if let Some(f) = &self.feed {
            f.set_pending(key, Some(size));
        }
        let mut m = self.dir_mtime.lock().unwrap();
        m.insert(parent_of(key), now_ms());
    }

    /// Creates an empty file (or truncates an existing one).
    pub async fn create(&self, parent: u64, name: &str) -> Result<Attr> {
        let p = self.dir_node(parent)?;
        check_name(name)?;
        let key = format!("{}{name}", p.key);
        if local_only(name) {
            let path = self.local_path(&key);
            if let Some(d) = path.parent() {
                fs::create_dir_all(d)?;
            }
            File::create(&path)?;
            return self.attr_of(&key, false).await;
        }
        self.ensure_dirty(&key, true).await?;
        self.touch_dirty(&key);
        self.attr_of(&key, false).await
    }

    /// Writes `data` at `offset`.
    pub async fn write(&self, ino: u64, offset: u64, data: &[u8]) -> Result<Attr> {
        let n = self.node(ino)?;
        if n.is_dir {
            return Err(Error::Invalid(format!("{} is a folder", n.key)));
        }
        let path = if local_only(name_of(&n.key)) {
            let p = self.local_path(&n.key);
            if let Some(d) = p.parent() {
                fs::create_dir_all(d)?;
            }
            p
        } else {
            self.ensure_dirty(&n.key, false).await?;
            self.dirty
                .lock()
                .unwrap()
                .get(&n.key)
                .map(|d| d.spool.clone())
                .ok_or_else(|| Error::NotFound(n.key.clone()))?
        };
        let data = data.to_vec();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let f = OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)?;
            write_at(&f, &data, offset)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Backend(e.to_string()))??;
        self.touch_dirty(&n.key);
        self.attr_of(&n.key, false).await
    }

    /// Sets the size (truncate or extend with zeros).
    pub async fn truncate(&self, ino: u64, size: u64) -> Result<Attr> {
        let n = self.node(ino)?;
        if n.is_dir {
            return Err(Error::Invalid(format!("{} is a folder", n.key)));
        }
        let path = if local_only(name_of(&n.key)) {
            self.local_path(&n.key)
        } else {
            self.ensure_dirty(&n.key, size == 0).await?;
            self.dirty
                .lock()
                .unwrap()
                .get(&n.key)
                .map(|d| d.spool.clone())
                .ok_or_else(|| Error::NotFound(n.key.clone()))?
        };
        OpenOptions::new().write(true).open(&path)?.set_len(size)?;
        self.touch_dirty(&n.key);
        self.attr_of(&n.key, false).await
    }

    /// Makes a folder.
    pub async fn mkdir(&self, parent: u64, name: &str) -> Result<Attr> {
        let p = self.dir_node(parent)?;
        check_name(name)?;
        if local_only(name) {
            return Err(Error::Forbidden(format!("{name}: not stored in the drive")));
        }
        let key = format!("{}{name}/", p.key);
        self.session().mkdir(&key).await?;
        self.attr_of(&key, true).await
    }

    /// Waits (bounded) until `key` has no upload in flight, so a rename or
    /// delete never races an upload that would bring the old name back.
    async fn settle(&self, key: &str) {
        for _ in 0..3000 {
            if !self
                .dirty
                .lock()
                .unwrap()
                .get(key)
                .is_some_and(|d| d.uploading)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Removes a file.
    pub async fn unlink(&self, parent: u64, name: &str) -> Result<()> {
        let p = self.dir_node(parent)?;
        let key = format!("{}{name}", p.key);
        if local_only(name) {
            fs::remove_file(self.local_path(&key))?;
            return Ok(());
        }
        self.settle(&key).await;
        let pending = self.dirty.lock().unwrap().remove(&key);
        if let Some(d) = &pending {
            let _ = fs::remove_file(&d.spool);
            if let Some(f) = &self.feed {
                f.set_pending(&key, None);
            }
        }
        match self.session().delete(&key, Condition::None).await {
            Ok(()) => Ok(()),
            // Only ever existed here.
            Err(Error::NotFound(_)) if pending.is_some() => {
                self.invalidate_key(&key, None);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Removes an empty folder.
    pub async fn rmdir(&self, parent: u64, name: &str) -> Result<()> {
        let p = self.dir_node(parent)?;
        let key = format!("{}{name}/", p.key);
        let has_pending = self
            .dirty
            .lock()
            .unwrap()
            .keys()
            .any(|k| k.starts_with(&key));
        if has_pending {
            return Err(Error::Precondition(format!("{key} is not empty")));
        }
        self.session().rmdir(&key).await?;
        self.invalidate_key(&key, None);
        Ok(())
    }

    /// Renames a file or folder (a copy and a delete in the store; a
    /// folder moves every file under it).
    pub async fn rename(&self, from_dir: u64, from: &str, to_dir: u64, to: &str) -> Result<()> {
        let fp = self.dir_node(from_dir)?;
        let tp = self.dir_node(to_dir)?;
        check_name(to)?;
        let src = format!("{}{from}", fp.key);
        let dst = format!("{}{to}", tp.key);
        if local_only(from) || local_only(to) {
            let (a, b) = (self.local_path(&src), self.local_path(&dst));
            if let Some(d) = b.parent() {
                fs::create_dir_all(d)?;
            }
            fs::rename(a, b)?;
            return Ok(());
        }
        self.settle(&src).await;
        // A file written here and not uploaded yet: move the spool.
        let moved = {
            let mut d = self.dirty.lock().unwrap();
            d.remove(&src).map(|x| {
                d.insert(dst.clone(), x);
            })
        };
        let session = self.session();
        let backend = session.drive().backend();
        if moved.is_some() {
            if let Some(f) = &self.feed {
                f.set_pending(&src, None);
            }
            self.touch_dirty(&dst);
            match session.delete(&src, Condition::None).await {
                Ok(()) | Err(Error::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
            self.flush(&dst).await?;
        } else if backend.head(&src).await?.is_some() {
            session.copy(&src, &dst).await?;
            session.delete(&src, Condition::None).await?;
        } else {
            // A folder: move everything under it.
            let from_folder = format!("{src}/");
            let to_folder = format!("{dst}/");
            let objects = backend.list(&from_folder).await?;
            if objects.is_empty() {
                return Err(Error::NotFound(src));
            }
            for m in objects {
                let rest = &m.key[from_folder.len()..];
                let target = format!("{to_folder}{rest}");
                if rest.ends_with(crate::drive::FOLDER_MARKER) {
                    session.mkdir(&parent_of(&target)).await?;
                    let _ = backend.delete(&m.key, Condition::None).await;
                    continue;
                }
                session.copy(&m.key, &target).await?;
                session.delete(&m.key, Condition::None).await?;
            }
            self.invalidate_key(&from_folder, None);
            self.invalidate_key(&to_folder, None);
        }
        // Keep inode numbers: the moved node (and its children) now name
        // the new keys.
        let mut ix = self.inodes.lock().unwrap();
        let moves: Vec<(String, u64)> = ix
            .by_key
            .iter()
            .filter(|(k, _)| **k == src || k.starts_with(&format!("{src}/")))
            .map(|(k, i)| (k.clone(), *i))
            .collect();
        for (old, ino) in moves {
            let new = format!("{dst}{}", &old[src.len()..]);
            ix.by_key.remove(&old);
            ix.by_key.insert(new.clone(), ino);
            if let Some(n) = ix.by_ino.get_mut(&ino) {
                n.key = new;
            }
        }
        Ok(())
    }

    /// Uploads `key`'s spool now (a close or an fsync). No-op when clean.
    pub async fn flush(&self, key: &str) -> Result<()> {
        let job = {
            let mut d = self.dirty.lock().unwrap();
            match d.get_mut(key) {
                Some(x) if !x.uploading => {
                    x.uploading = true;
                    Some((x.spool.clone(), x.base_etag.clone(), x.generation))
                }
                // Already on its way: wait for it below.
                Some(_) => None,
                None => return Ok(()),
            }
        };
        let Some((spool, base, generation)) = job else {
            // Wait (bounded) for the running upload.
            for _ in 0..600 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if !self
                    .dirty
                    .lock()
                    .unwrap()
                    .get(key)
                    .is_some_and(|x| x.uploading)
                {
                    break;
                }
            }
            return Ok(());
        };
        let r = self.upload(key, &spool, base.as_deref()).await;
        let mut d = self.dirty.lock().unwrap();
        match r {
            Ok(meta) => {
                let clean = d.get(key).is_some_and(|x| x.generation == generation);
                if clean {
                    if let Some(x) = d.remove(key) {
                        let _ = fs::remove_file(x.spool);
                    }
                    if let Some(f) = &self.feed {
                        f.set_pending(key, None);
                    }
                } else if let Some(x) = d.get_mut(key) {
                    // Written again during the upload: base the next upload
                    // on what just landed.
                    x.uploading = false;
                    x.base_etag = Some(meta.etag.clone());
                }
                drop(d);
                self.invalidate_key(key, Some(meta));
                Ok(())
            }
            Err(e) => {
                if let Some(x) = d.get_mut(key) {
                    x.uploading = false;
                    x.error = Some(e.to_string());
                }
                drop(d);
                if let Some(f) = &self.feed {
                    f.event("upload_failed", key, "", 0, "", &e.to_string());
                }
                Err(e)
            }
        }
    }

    async fn upload(&self, key: &str, spool: &Path, base: Option<&str>) -> Result<ObjectMeta> {
        let session = self.session();
        let backend = session.drive().backend();
        let size = fs::metadata(spool).map(|m| m.len()).unwrap_or(0);
        if let Some(f) = &self.feed {
            f.event("upload_started", key, "", size, "", "");
        }
        let current = backend.head(key).await?;
        // Changed in the store since this spool was based on it: keep the
        // store's version visibly, then land ours (last writer wins).
        if let Some(cur) = &current
            && base != Some(cur.etag.as_str())
        {
            let feed = self.feed.as_ref();
            let other = feed
                .and_then(|f| f.last_writer(key))
                .unwrap_or_else(|| "another device".into());
            let other_name = feed.map(|f| f.device_name(&other)).unwrap_or(other.clone());
            let copy = conflict_path(key, &other_name, now_ms());
            session.copy(key, &copy).await?;
            let me = feed.map(|f| f.device().id.clone()).unwrap_or_default();
            let meta = session.write_file(key, spool, Condition::None).await?;
            if let Some(f) = feed {
                f.conflict(Conflict {
                    path: key.to_string(),
                    conflict_path: copy,
                    winner_device: me,
                    loser_device: other,
                    winner_version: meta.version.clone(),
                    loser_version: cur.version.clone(),
                    ts_ms: now_ms(),
                });
                f.event("upload_done", key, "", meta.size, &meta.version, "conflict");
            }
            return Ok(meta);
        }
        let meta = session.write_file(key, spool, Condition::None).await?;
        if let Some(f) = &self.feed {
            f.event("upload_done", key, "", meta.size, &meta.version, "");
        }
        Ok(meta)
    }

    /// Uploads every spool idle for [`UPLOAD_IDLE`] (call on a timer from
    /// mounts that have no close). Returns how many uploaded.
    pub async fn upload_idle(&self) -> usize {
        let ready: Vec<String> = self
            .dirty
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, d)| {
                !d.uploading
                    && d.last_write.elapsed() >= UPLOAD_IDLE
                    && (d.error.is_none() || d.last_write.elapsed() >= UPLOAD_IDLE * 20)
            })
            .map(|(k, _)| k.clone())
            .collect();
        let mut n = 0;
        for k in ready {
            if self.flush(&k).await.is_ok() {
                n += 1;
            }
        }
        n
    }

    /// Uploads everything pending (unmount, shutdown).
    pub async fn flush_all(&self) -> Result<()> {
        let keys: Vec<String> = self.dirty.lock().unwrap().keys().cloned().collect();
        let mut first_err = None;
        for k in keys {
            if let Err(e) = self.flush(&k).await {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    /// Files waiting to upload: `(key, bytes)`.
    pub fn pending(&self) -> Vec<(String, u64)> {
        self.dirty
            .lock()
            .unwrap()
            .iter()
            .map(|(k, d)| {
                (
                    k.clone(),
                    fs::metadata(&d.spool).map(|m| m.len()).unwrap_or(0),
                )
            })
            .collect()
    }

    /// The drive this filesystem serves.
    pub fn drive(&self) -> &Drive {
        &self.drive
    }
}

fn mtime_ms(m: &fs::Metadata) -> u64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn read_file_range(path: &Path, offset: u64, len: u64) -> Result<(Vec<u8>, bool)> {
    let f = File::open(path)?;
    let size = f.metadata()?.len();
    if offset >= size {
        return Ok((vec![], true));
    }
    let want = len.min(size - offset) as usize;
    let mut buf = vec![0u8; want];
    let mut done = 0;
    while done < want {
        let n = read_at(&f, &mut buf[done..], offset + done as u64)?;
        if n == 0 {
            break;
        }
        done += n;
    }
    buf.truncate(done);
    Ok((buf, offset + done as u64 >= size))
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, offset)
}

#[cfg(unix)]
fn write_at(f: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(f, buf, offset)
}

#[cfg(windows)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, offset)
}

#[cfg(windows)]
fn write_at(f: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        done += std::os::windows::fs::FileExt::seek_write(f, &buf[done..], offset + done as u64)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::DeviceId;
    use crate::fs::FsBackend;

    fn home(dir: &Path, name: &str, data: &Path) -> (Drive, Arc<Feed>, Arc<Vfs>) {
        let state = dir.join(name);
        let d = Drive::new(Arc::new(FsBackend::new(data)), &state);
        let f = Feed::new(
            &d,
            DeviceId {
                id: format!("dev{name}"),
                name: name.to_uppercase(),
            },
        );
        let v = Vfs::new(&d, Context::user(), None, Some(f.clone()), &state).unwrap();
        (d, f, v)
    }

    async fn find(v: &Vfs, path: &str) -> Result<Attr> {
        let mut ino = ROOT;
        let mut attr = v.getattr(ROOT).await?;
        for c in path.split('/').filter(|c| !c.is_empty()) {
            attr = v.lookup(ino, c).await?;
            ino = attr.ino;
        }
        Ok(attr)
    }

    #[tokio::test]
    async fn a_space_volume_becomes_its_agents_view_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let d = Drive::new(
            Arc::new(FsBackend::new(dir.path().join("data"))),
            dir.path().join("state"),
        );
        let v = Vfs::new(
            &d,
            Context::space("local:lab"),
            None,
            None,
            &dir.path().join("m"),
        )
        .unwrap();
        let names = |v: Arc<Vfs>| async move {
            v.readdir(ROOT)
                .await
                .unwrap()
                .into_iter()
                .map(|(n, _)| n)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(v.clone()).await, ["public", "spaces"]);
        // A spooled Space write lands as the Space before the switch.
        let folder = find(&v, "spaces").await.unwrap();
        let lab = v.readdir(folder.ino).await.unwrap()[0].1.clone();
        let f = v.create(lab.ino, "space.txt").await.unwrap();
        v.write(f.ino, 0, b"from the space").await.unwrap();
        v.set_context(Context::agent("ada", Some("local:lab")))
            .await
            .unwrap();
        assert!(v.pending().is_empty());
        let (events, _) = d.audit().tail(50).unwrap();
        let wrote = events
            .iter()
            .find(|e| e.path == "spaces/local-lab/space.txt" && e.action == "write")
            .expect("the Space's write is audited");
        assert_eq!(wrote.principal, "space:local-lab");
        assert_eq!(names(v.clone()).await, ["agents", "public", "spaces"]);
        let home = find(&v, "agents/ada").await.unwrap();
        let n = v.create(home.ino, "memory.md").await.unwrap();
        v.write(n.ino, 0, b"remember").await.unwrap();
        v.set_context(Context::space("local:lab")).await.unwrap();
        assert_eq!(
            d.session(Context::agent("ada", None))
                .read("agents/ada/memory.md", None)
                .await
                .unwrap()
                .0,
            b"remember"
        );
        assert_eq!(names(v.clone()).await, ["public", "spaces"]);
        assert!(find(&v, "agents/ada").await.is_err());
    }

    #[tokio::test]
    async fn files_round_trip_through_the_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let (d, _f, v) = home(dir.path(), "a", &dir.path().join("data"));
        let public = find(&v, "public").await.unwrap();
        assert!(public.is_dir);
        let a = v.create(public.ino, "notes.md").await.unwrap();
        v.write(a.ino, 0, b"hello ").await.unwrap();
        v.write(a.ino, 6, b"world").await.unwrap();
        // Visible before upload, from the spool.
        assert_eq!(
            v.read(a.ino, 0, 100).await.unwrap(),
            (b"hello world".to_vec(), true)
        );
        assert_eq!(v.pending().len(), 1);
        v.flush("public/notes.md").await.unwrap();
        assert!(v.pending().is_empty());
        let s = d.session(Context::user());
        assert_eq!(
            s.read("public/notes.md", None).await.unwrap().0,
            b"hello world"
        );
        // A partial overwrite of an uploaded file copies it in first.
        v.write(a.ino, 0, b"HELLO").await.unwrap();
        v.flush("public/notes.md").await.unwrap();
        assert_eq!(
            s.read("public/notes.md", None).await.unwrap().0,
            b"HELLO world"
        );
        assert_eq!(s.history("public/notes.md").await.unwrap().len(), 2);
        // Read through the store (not the spool) at an offset.
        assert_eq!(v.read(a.ino, 6, 3).await.unwrap(), (b"wor".to_vec(), false));
        // Folders, listings, renames (inode kept), removal.
        let docs = v.mkdir(public.ino, "docs").await.unwrap();
        let names: Vec<String> = v
            .readdir(public.ino)
            .await
            .unwrap()
            .into_iter()
            .map(|x| x.0)
            .collect();
        assert_eq!(names, ["docs", "notes.md"]);
        v.rename(public.ino, "notes.md", docs.ino, "moved.md")
            .await
            .unwrap();
        assert_eq!(v.key_of(a.ino).unwrap(), "public/docs/moved.md");
        assert_eq!(v.read(a.ino, 0, 5).await.unwrap().0, b"HELLO");
        assert!(find(&v, "public/notes.md").await.is_err());
        v.rename(public.ino, "docs", public.ino, "docs2")
            .await
            .unwrap();
        assert_eq!(v.key_of(a.ino).unwrap(), "public/docs2/moved.md");
        assert_eq!(
            s.read("public/docs2/moved.md", None).await.unwrap().0,
            b"HELLO world"
        );
        let docs2 = find(&v, "public/docs2").await.unwrap();
        assert_eq!(
            errno(&v.rmdir(public.ino, "docs2").await.unwrap_err()),
            libc_errno::ENOTEMPTY
        );
        v.unlink(docs2.ino, "moved.md").await.unwrap();
        v.rmdir(public.ino, "docs2").await.unwrap();
        assert!(v.readdir(public.ino).await.unwrap().is_empty());
        // Finder metadata stays local; reserved names are refused.
        let ds = v.create(public.ino, ".DS_Store").await.unwrap();
        v.write(ds.ino, 0, b"finder").await.unwrap();
        assert!(v.pending().is_empty());
        assert!(s.read("public/.DS_Store", None).await.is_err());
        assert_eq!(v.readdir(public.ino).await.unwrap()[0].0, ".DS_Store");
        assert_eq!(
            errno(&v.create(public.ino, ".cua-lease").await.unwrap_err()),
            libc_errno::EINVAL
        );
    }

    #[tokio::test]
    async fn agents_see_their_rules_and_secrets_are_refused_at_upload() {
        let dir = tempfile::tempdir().unwrap();
        let d = Drive::open_local(dir.path());
        let v = Vfs::new(
            &d,
            Context::agent("ada", None),
            None,
            None,
            &dir.path().join("m"),
        )
        .unwrap();
        let agents = find(&v, "agents").await.unwrap();
        let names: Vec<String> = v
            .readdir(agents.ino)
            .await
            .unwrap()
            .into_iter()
            .map(|x| x.0)
            .collect();
        assert_eq!(names, ["ada"]);
        let public = find(&v, "public").await.unwrap();
        assert_eq!(
            errno(&v.create(public.ino, "x").await.unwrap_err()),
            libc_errno::EACCES
        );
        let home = find(&v, "agents/ada").await.unwrap();
        let f = v.create(home.ino, "MEMORY.md").await.unwrap();
        let leak = format!("key {}{}\n", "AKIA", "ABCDEFGHIJKLMNOP");
        v.write(f.ino, 0, leak.as_bytes()).await.unwrap();
        let e = v.flush("agents/ada/MEMORY.md").await.unwrap_err();
        assert_eq!(e.tag(), "secret_detected");
        // Kept locally (never silently dropped) until fixed.
        assert_eq!(v.pending().len(), 1);
        v.truncate(f.ino, 0).await.unwrap();
        v.write(f.ino, 0, b"likes tea").await.unwrap();
        v.flush("agents/ada/MEMORY.md").await.unwrap();
        assert!(v.pending().is_empty());
    }

    #[tokio::test]
    async fn a_concurrent_remote_write_becomes_a_visible_conflict_copy() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("shared");
        let (da, fa, va) = home(dir.path(), "a", &data);
        let (_db, fb, vb) = home(dir.path(), "b", &data);
        fa.spawn();
        fb.spawn();
        let s = da.session(Context::user());
        s.write("public/plan.md", b"v1".to_vec(), Condition::None)
            .await
            .unwrap();
        // B opens and edits (based on v1)...
        let fbid = find(&vb, "public/plan.md").await.unwrap();
        vb.write(fbid.ino, 0, b"B2").await.unwrap();
        // ...while A lands v2 first.
        let faid = find(&va, "public/plan.md").await.unwrap();
        va.write(faid.ino, 0, b"A2").await.unwrap();
        va.flush("public/plan.md").await.unwrap();
        // B's upload finds the store changed: A's content is kept visibly.
        vb.flush("public/plan.md").await.unwrap();
        assert_eq!(s.read("public/plan.md", None).await.unwrap().0, b"B2");
        let st = fb.status();
        assert_eq!(st.conflicts.len(), 1, "{st:?}");
        let c = &st.conflicts[0];
        assert!(
            c.conflict_path.starts_with("public/plan (conflict from "),
            "{c:?}"
        );
        assert_eq!(s.read(&c.conflict_path, None).await.unwrap().0, b"A2");
        // Each file says so.
        let f = fb.file_sync("public/plan.md");
        assert_eq!(
            (f.state.as_str(), f.conflict_path.as_deref()),
            ("conflict", Some(c.conflict_path.as_str()))
        );
        assert_eq!(fb.file_sync(&c.conflict_path).state, "conflict_copy");
        // Nothing lost: all three versions are in the history.
        assert_eq!(s.history("public/plan.md").await.unwrap().len(), 3);
        // A sees B's write through the feed within seconds.
        let t0 = Instant::now();
        loop {
            let a = find(&va, "public/plan.md").await.unwrap();
            if va.read(a.ino, 0, 10).await.unwrap().0 == b"B2" {
                break;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(8),
                "A never saw B's write"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        fa.stop();
        fb.stop();
    }
}
