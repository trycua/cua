// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The drive as a FUSE filesystem on Linux (feature `fuse`), through
//! `/dev/fuse` and the setuid `fusermount3` helper, with no libfuse.
//!
//! Every request is answered from a Tokio task over the shared [`Vfs`], so
//! a slow remote read never blocks other requests. Files upload when the
//! last handle closes (`release`) or on `fsync`, the same rules as the
//! macOS mount.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use fuser::{
    BackgroundSession, Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags,
    Generation, INodeNo, LockOwner, MountOption, OpenFlags, RenameFlags, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request,
    TimeOrNow, WriteFlags,
};

use crate::vfs::{Attr, FsOps, errno};
use crate::{Error, Result};

/// How long the kernel may cache attributes and lookups. Short: the change
/// feed invalidates the daemon's view within seconds, and the kernel's
/// must follow.
const TTL: Duration = Duration::from_secs(1);

struct DriveFs {
    vfs: Arc<dyn FsOps>,
    rt: tokio::runtime::Handle,
    uid: u32,
    gid: u32,
}

fn to_errno(e: &Error) -> Errno {
    Errno::from_i32(errno(e))
}

fn name(n: &OsStr) -> std::result::Result<String, Errno> {
    n.to_str().map(str::to_string).ok_or(Errno::EINVAL)
}

fn file_attr(a: &Attr, uid: u32, gid: u32) -> FileAttr {
    let t = UNIX_EPOCH + Duration::from_millis(a.mtime_ms);
    let perm = match (a.is_dir, a.writable) {
        (true, true) => 0o755,
        (true, false) => 0o555,
        (false, true) => 0o644,
        (false, false) => 0o444,
    };
    FileAttr {
        ino: INodeNo(a.ino),
        size: a.size,
        blocks: a.size.div_ceil(512),
        atime: t,
        mtime: t,
        ctime: t,
        crtime: t,
        kind: if a.is_dir {
            FileType::Directory
        } else {
            FileType::RegularFile
        },
        perm,
        nlink: if a.is_dir { 2 } else { 1 },
        uid,
        gid,
        rdev: 0,
        blksize: 1 << 20,
        flags: 0,
    }
}

impl Filesystem for DriveFs {
    fn lookup(&self, _req: &Request, parent: INodeNo, n: &OsStr, reply: ReplyEntry) {
        let Ok(n) = name(n) else {
            return reply.error(Errno::EINVAL);
        };
        let (vfs, me) = (self.vfs.clone(), self.clone_attrs());
        self.rt.spawn(async move {
            match vfs.lookup(parent.0, &n).await {
                Ok(a) => reply.entry(&TTL, &me.attr(&a), Generation(0)),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let (vfs, me) = (self.vfs.clone(), self.clone_attrs());
        self.rt.spawn(async move {
            match vfs.getattr(ino.0).await {
                Ok(a) => reply.attr(&TTL, &me.attr(&a)),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let (vfs, me) = (self.vfs.clone(), self.clone_attrs());
        self.rt.spawn(async move {
            let r = match size {
                Some(s) => vfs.truncate(ino.0, s).await,
                None => vfs.getattr(ino.0).await,
            };
            match r {
                Ok(a) => reply.attr(&TTL, &me.attr(&a)),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        n: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        let Ok(n) = name(n) else {
            return reply.error(Errno::EINVAL);
        };
        let (vfs, me) = (self.vfs.clone(), self.clone_attrs());
        self.rt.spawn(async move {
            match vfs.mkdir(parent.0, &n).await {
                Ok(a) => reply.entry(&TTL, &me.attr(&a), Generation(0)),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, n: &OsStr, reply: ReplyEmpty) {
        let Ok(n) = name(n) else {
            return reply.error(Errno::EINVAL);
        };
        let vfs = self.vfs.clone();
        self.rt.spawn(async move {
            match vfs.unlink(parent.0, &n).await {
                Ok(()) => reply.ok(),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, n: &OsStr, reply: ReplyEmpty) {
        let Ok(n) = name(n) else {
            return reply.error(Errno::EINVAL);
        };
        let vfs = self.vfs.clone();
        self.rt.spawn(async move {
            match vfs.rmdir(parent.0, &n).await {
                Ok(()) => reply.ok(),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        n: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        _flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        let (Ok(from), Ok(to)) = (name(n), name(newname)) else {
            return reply.error(Errno::EINVAL);
        };
        let vfs = self.vfs.clone();
        self.rt.spawn(async move {
            // Replacing an existing file (an editor's safe save).
            if let Ok(a) = vfs.lookup(newparent.0, &to).await
                && !a.is_dir
            {
                let _ = vfs.unlink(newparent.0, &to).await;
            }
            match vfs.rename(parent.0, &from, newparent.0, &to).await {
                Ok(()) => reply.ok(),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    fn open(&self, _req: &Request, _ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        reply.opened(FileHandle(0), FopenFlags::empty());
    }

    #[allow(clippy::too_many_arguments)]
    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let vfs = self.vfs.clone();
        self.rt.spawn(async move {
            match vfs.read(ino.0, offset, size as u64).await {
                Ok((bytes, _)) => reply.data(&bytes),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn write(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        let (vfs, data) = (self.vfs.clone(), data.to_vec());
        self.rt.spawn(async move {
            match vfs.write(ino.0, offset, &data).await {
                Ok(_) => reply.written(data.len() as u32),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    fn flush(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        _lo: LockOwner,
        reply: ReplyEmpty,
    ) {
        // A close: writes held back on this side go now, and one that
        // failed after it was acknowledged fails the close.
        let vfs = self.vfs.clone();
        self.rt.spawn(async move {
            match vfs.sync_writes(ino.0).await {
                Ok(()) => reply.ok(),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    fn release(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let vfs = self.vfs.clone();
        self.rt.spawn(async move {
            // Errors are reported as sync events; the spool is kept.
            let _ = vfs.flush_ino(ino.0).await;
            reply.ok();
        });
    }

    fn fsync(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        let vfs = self.vfs.clone();
        self.rt.spawn(async move {
            match vfs.flush_ino(ino.0).await {
                Ok(()) => reply.ok(),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let vfs = self.vfs.clone();
        self.rt.spawn(async move {
            match vfs.readdir(ino.0).await {
                Ok(entries) => {
                    let mut all = vec![
                        (ino.0, FileType::Directory, ".".to_string()),
                        (ino.0, FileType::Directory, "..".to_string()),
                    ];
                    for (n, a) in entries {
                        let kind = if a.is_dir {
                            FileType::Directory
                        } else {
                            FileType::RegularFile
                        };
                        all.push((a.ino, kind, n));
                    }
                    for (i, (ino, kind, n)) in all.into_iter().enumerate().skip(offset as usize) {
                        if reply.add(INodeNo(ino), (i + 1) as u64, kind, n) {
                            break;
                        }
                    }
                    reply.ok();
                }
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        n: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        let Ok(n) = name(n) else {
            return reply.error(Errno::EINVAL);
        };
        let (vfs, me) = (self.vfs.clone(), self.clone_attrs());
        self.rt.spawn(async move {
            match vfs.create(parent.0, &n).await {
                Ok(a) => reply.created(
                    &TTL,
                    &me.attr(&a),
                    Generation(0),
                    FileHandle(0),
                    FopenFlags::empty(),
                ),
                Err(e) => reply.error(to_errno(&e)),
            }
        });
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        // A large, mostly free volume: the store has no fixed size.
        let blocks = 1u64 << 40;
        reply.statfs(
            blocks,
            blocks / 2,
            blocks / 2,
            1 << 30,
            1 << 29,
            4096,
            1024,
            4096,
        );
    }
}

/// The attribute builder, cloned into tasks.
#[derive(Clone, Copy)]
struct Attrs {
    uid: u32,
    gid: u32,
}

impl Attrs {
    fn attr(&self, a: &Attr) -> FileAttr {
        file_attr(a, self.uid, self.gid)
    }
}

impl DriveFs {
    fn clone_attrs(&self) -> Attrs {
        Attrs {
            uid: self.uid,
            gid: self.gid,
        }
    }
}

/// A mounted FUSE filesystem; unmounted by [`FuseMount::unmount`] (or on
/// drop).
pub struct FuseMount {
    session: Option<BackgroundSession>,
    vfs: Arc<dyn FsOps>,
    path: std::path::PathBuf,
}

impl std::fmt::Debug for FuseMount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FuseMount")
    }
}

impl FuseMount {
    /// Mounts `vfs` at `path` (an empty folder the user owns). Through
    /// `fusermount3` when not root. `allow_other` lets other users of the
    /// machine use the mount (a Space's agent user, when spacesd mounts).
    pub fn mount(vfs: Arc<dyn FsOps>, path: &Path) -> Result<FuseMount> {
        Self::mount_with(vfs, path, false, "cua-volume", None)
    }

    /// [`FuseMount::mount`] with `allow_other`, a source name, and the
    /// owner files are reported as (`(uid, gid)`; default: this process).
    /// Access is checked by the kernel against these modes and the owner
    /// (`default_permissions`), and by the volume's rules behind them: a
    /// root helper mounting for the desktop user passes that user, or the
    /// user could not write where the rules allow.
    pub fn mount_with(
        vfs: Arc<dyn FsOps>,
        path: &Path,
        allow_other: bool,
        source: &str,
        owner: Option<(u32, u32)>,
    ) -> Result<FuseMount> {
        // SAFETY: getuid/getgid never fail.
        let (uid, gid) = owner.unwrap_or_else(|| unsafe { (libc::getuid(), libc::getgid()) });
        let fs = DriveFs {
            vfs: vfs.clone(),
            rt: tokio::runtime::Handle::current(),
            uid,
            gid,
        };
        let mut config = Config::default();
        config.mount_options = vec![
            MountOption::FSName(source.into()),
            MountOption::Subtype("cua-volume".into()),
            MountOption::RW,
            MountOption::NoAtime,
            MountOption::NoDev,
            MountOption::NoSuid,
            MountOption::DefaultPermissions,
        ];
        if allow_other {
            config.acl = fuser::SessionACL::All;
        }
        config.n_threads = Some(4);
        let session = fuser::spawn_mount(fs, path, &config)
            .map_err(|e| Error::Backend(format!("fuse mount {}: {e}", path.display())))?;
        Ok(FuseMount {
            session: Some(session),
            vfs,
            path: path.to_path_buf(),
        })
    }

    /// Uploads what is pending, then unmounts. The kernel side is
    /// confirmed through `/proc/self/mountinfo`: some kernels (gVisor) never
    /// wake the worker thread after an unmount, so its join is bounded and a
    /// stuck worker is left to end with the process.
    pub async fn unmount(mut self) -> Result<()> {
        let flushed = self.vfs.flush_all().await;
        let path = self.path.clone();
        if let Some(s) = self.session.take() {
            // A plain thread, not the runtime's blocking pool: a runtime
            // waits for its blocking tasks when it shuts down.
            let (tx, rx) = tokio::sync::oneshot::channel();
            std::thread::spawn(move || {
                let _ = tx.send(s.umount_and_join());
            });
            match tokio::time::timeout(Duration::from_secs(3), rx).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => {
                    if is_mounted(&path) {
                        return Err(Error::Backend(format!("fuse unmount: {e}")));
                    }
                }
                Ok(Err(_)) => return Err(Error::Backend("fuse unmount thread died".into())),
                Err(_) => {
                    if is_mounted(&path) {
                        lazy_unmount(&path);
                    }
                    if is_mounted(&path) {
                        return Err(Error::Backend(format!(
                            "{} did not unmount",
                            path.display()
                        )));
                    }
                }
            }
        }
        flushed
    }
}

/// Whether `path` is a mount point right now.
pub fn is_mounted(path: &Path) -> bool {
    let Ok(info) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return false;
    };
    let want = path.to_string_lossy();
    info.lines().any(|l| {
        l.split(' ')
            .nth(4)
            .map(|m| m.replace("\\040", " ") == want)
            .unwrap_or(false)
    })
}

/// Detaches a mount that is still busy (`umount -l`, then `fusermount3 -uz`).
pub fn lazy_unmount(path: &Path) {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    if let Ok(c) = CString::new(path.as_os_str().as_bytes()) {
        // SAFETY: a valid NUL-terminated path.
        unsafe { libc::umount2(c.as_ptr(), libc::MNT_DETACH) };
    }
    if is_mounted(path) {
        let _ = std::process::Command::new("fusermount3")
            .arg("-uz")
            .arg(path)
            .status();
    }
}
