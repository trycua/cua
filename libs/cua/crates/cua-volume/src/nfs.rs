// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The drive as a localhost NFSv3 server (feature `nfs`), which macOS
//! mounts with its built-in client: no kernel extension, no system
//! extension, no approval step. This is the macOS mount until the FSKit
//! module ships signed (see the Cua Volume guide).
//!
//! The server binds `127.0.0.1` on a random port and serves one
//! [`Vfs`]. NFSv3 has no open or close, so a written file uploads after
//! [`crate::vfs::UPLOAD_IDLE`] without writes; unmounting (or stopping
//! the server) uploads whatever is left first.

use std::sync::Arc;
use std::time::Duration;

use nfsserve::nfs::{
    cookieverf3, fattr3, fileid3, filename3, fsinfo3, ftype3, nfs_fh3, nfspath3, nfsstat3,
    nfstime3, sattr3, set_size3, specdata3,
};
use nfsserve::tcp::{NFSTcp, NFSTcpListener};
use nfsserve::vfs::{DirEntry, NFSFileSystem, ReadDirResult, VFSCapabilities};

use crate::vfs::{Attr, Vfs, errno, libc_errno};
use crate::{Error, Result};

/// The first component of a per-mount export name: `/cua-volume-<id>`
/// mounts the root like `/` does. A guest names each mount this way (with
/// its own id) so a client that caches by server and export (Windows keeps
/// the root handle and directory listings of `\\\\127.0.0.1\\<export>`
/// across mounts) never reuses an earlier view's.
pub const EXPORT_PREFIX: &str = "cua-volume-";

/// Largest read or write the client is told to use.
const IO_SIZE: u32 = 1024 * 1024;

struct NfsFs {
    vfs: Arc<Vfs>,
    uid: u32,
    gid: u32,
    /// Writable entries are writable by anyone (a guest's client, whose
    /// uid is not the host's; the access rules are enforced here).
    shared: bool,
    /// This server's generation: the first half of every file handle it
    /// hands out, and its file system id. Unique per server in this process
    /// (nfsserve's default is one per process), so a client never mistakes
    /// one view's handles or cached listings for another's: two Spaces, or
    /// a Space's view after an agent's, get different handles for the same
    /// file.
    generation: u64,
}

/// A generation no other server in this process has had: the time in
/// milliseconds, raised past the last one handed out.
fn next_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut last = LAST.load(Ordering::SeqCst);
    loop {
        let next = now.max(last + 1);
        match LAST.compare_exchange(last, next, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return next,
            Err(seen) => last = seen,
        }
    }
}

fn stat(e: Error) -> nfsstat3 {
    match errno(&e) {
        libc_errno::ENOENT => nfsstat3::NFS3ERR_NOENT,
        libc_errno::EACCES => nfsstat3::NFS3ERR_ACCES,
        libc_errno::EEXIST => nfsstat3::NFS3ERR_EXIST,
        libc_errno::ENOTEMPTY => nfsstat3::NFS3ERR_NOTEMPTY,
        libc_errno::EINVAL => nfsstat3::NFS3ERR_INVAL,
        libc_errno::ENOTDIR => nfsstat3::NFS3ERR_NOTDIR,
        libc_errno::EISDIR => nfsstat3::NFS3ERR_ISDIR,
        _ => nfsstat3::NFS3ERR_IO,
    }
}

fn name(f: &filename3) -> std::result::Result<String, nfsstat3> {
    String::from_utf8(f.0.clone()).map_err(|_| nfsstat3::NFS3ERR_INVAL)
}

fn time(ms: u64) -> nfstime3 {
    nfstime3 {
        seconds: (ms / 1000) as u32,
        nseconds: ((ms % 1000) * 1_000_000) as u32,
    }
}

impl NfsFs {
    /// The file system id this server reports: distinct per server, and
    /// below 2^31. The generation itself (milliseconds since 1970, past
    /// 2^40) made the Windows NFS client refuse every rename on the mount
    /// as a move to another drive (ERROR_NOT_SAME_DEVICE) without asking
    /// the server, as if it keeps the fsid in 32 bits in one place and 64
    /// in another.
    fn fsid(&self) -> u64 {
        self.generation % 0x7fff_fff0 + 1
    }

    fn fattr(&self, a: &Attr) -> fattr3 {
        let mode = match (a.is_dir, a.writable, self.shared) {
            (true, true, true) => 0o777,
            (true, true, false) => 0o755,
            (true, false, _) => 0o555,
            (false, true, true) => 0o666,
            (false, true, false) => 0o644,
            (false, false, _) => 0o444,
        };
        fattr3 {
            ftype: if a.is_dir {
                ftype3::NF3DIR
            } else {
                ftype3::NF3REG
            },
            mode,
            nlink: if a.is_dir { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            size: a.size,
            used: a.size,
            rdev: specdata3::default(),
            fsid: self.fsid(),
            fileid: a.ino,
            atime: time(a.mtime_ms),
            mtime: time(a.mtime_ms),
            ctime: time(a.mtime_ms),
        }
    }
}

#[async_trait::async_trait]
impl NFSFileSystem for NfsFs {
    fn capabilities(&self) -> VFSCapabilities {
        VFSCapabilities::ReadWrite
    }

    fn root_dir(&self) -> fileid3 {
        crate::vfs::ROOT
    }

    /// MOUNT's path: `/`, or `/cua-volume-<id>` for the root (see
    /// [`EXPORT_PREFIX`]), followed by an optional subpath.
    async fn path_to_id(&self, path: &[u8]) -> std::result::Result<fileid3, nfsstat3> {
        let mut parts: Vec<Vec<u8>> = path
            .split(|&b| b == b'/')
            .filter(|p| !p.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        if parts
            .first()
            .is_some_and(|p| p.starts_with(EXPORT_PREFIX.as_bytes()))
        {
            parts.remove(0);
        }
        let mut id = self.root_dir();
        for part in parts {
            id = self.lookup(id, &part.into()).await?;
        }
        Ok(id)
    }

    fn id_to_fh(&self, id: fileid3) -> nfs_fh3 {
        let mut data = Vec::with_capacity(16);
        data.extend_from_slice(&self.generation.to_le_bytes());
        data.extend_from_slice(&id.to_le_bytes());
        nfs_fh3 { data }
    }

    fn fh_to_id(&self, fh: &nfs_fh3) -> std::result::Result<fileid3, nfsstat3> {
        let (Some(generation), Some(id)) = (fh.data.get(..8), fh.data.get(8..16)) else {
            return Err(nfsstat3::NFS3ERR_BADHANDLE);
        };
        if fh.data.len() != 16 {
            return Err(nfsstat3::NFS3ERR_BADHANDLE);
        }
        let generation = u64::from_le_bytes(generation.try_into().expect("8 bytes"));
        match generation.cmp(&self.generation) {
            // Another server's handle (an earlier view, or a restart).
            std::cmp::Ordering::Less => Err(nfsstat3::NFS3ERR_STALE),
            std::cmp::Ordering::Greater => Err(nfsstat3::NFS3ERR_BADHANDLE),
            std::cmp::Ordering::Equal => Ok(u64::from_le_bytes(id.try_into().expect("8 bytes"))),
        }
    }

    fn serverid(&self) -> cookieverf3 {
        self.generation.to_le_bytes()
    }

    async fn lookup(
        &self,
        dirid: fileid3,
        filename: &filename3,
    ) -> std::result::Result<fileid3, nfsstat3> {
        let n = name(filename)?;
        if n == "." {
            return Ok(dirid);
        }
        if n == ".." {
            let key = self.vfs.key_of(dirid).map_err(stat)?;
            let parent = parent_key(&key);
            if parent.is_empty() {
                return Ok(crate::vfs::ROOT);
            }
            return self
                .vfs
                .lookup_path(&parent)
                .await
                .map(|a| a.ino)
                .map_err(stat);
        }
        self.vfs
            .lookup(dirid, &n)
            .await
            .map(|a| a.ino)
            .map_err(stat)
    }

    async fn getattr(&self, id: fileid3) -> std::result::Result<fattr3, nfsstat3> {
        self.vfs
            .getattr(id)
            .await
            .map(|a| self.fattr(&a))
            .map_err(stat)
    }

    async fn setattr(&self, id: fileid3, setattr: sattr3) -> std::result::Result<fattr3, nfsstat3> {
        let a = match setattr.size {
            set_size3::size(s) => self.vfs.truncate(id, s).await.map_err(stat)?,
            set_size3::Void => self.vfs.getattr(id).await.map_err(stat)?,
        };
        Ok(self.fattr(&a))
    }

    async fn read(
        &self,
        id: fileid3,
        offset: u64,
        count: u32,
    ) -> std::result::Result<(Vec<u8>, bool), nfsstat3> {
        self.vfs.read(id, offset, count as u64).await.map_err(stat)
    }

    async fn write(
        &self,
        id: fileid3,
        offset: u64,
        data: &[u8],
    ) -> std::result::Result<fattr3, nfsstat3> {
        self.vfs
            .write(id, offset, data)
            .await
            .map(|a| self.fattr(&a))
            .map_err(stat)
    }

    async fn create(
        &self,
        dirid: fileid3,
        filename: &filename3,
        attr: sattr3,
    ) -> std::result::Result<(fileid3, fattr3), nfsstat3> {
        let a = self
            .vfs
            .create(dirid, &name(filename)?)
            .await
            .map_err(stat)?;
        let a = match attr.size {
            set_size3::size(s) if s > 0 => self.vfs.truncate(a.ino, s).await.map_err(stat)?,
            _ => a,
        };
        Ok((a.ino, self.fattr(&a)))
    }

    async fn create_exclusive(
        &self,
        dirid: fileid3,
        filename: &filename3,
    ) -> std::result::Result<fileid3, nfsstat3> {
        let n = name(filename)?;
        if self.vfs.lookup(dirid, &n).await.is_ok() {
            return Err(nfsstat3::NFS3ERR_EXIST);
        }
        self.vfs
            .create(dirid, &n)
            .await
            .map(|a| a.ino)
            .map_err(stat)
    }

    async fn mkdir(
        &self,
        dirid: fileid3,
        dirname: &filename3,
    ) -> std::result::Result<(fileid3, fattr3), nfsstat3> {
        let a = self.vfs.mkdir(dirid, &name(dirname)?).await.map_err(stat)?;
        Ok((a.ino, self.fattr(&a)))
    }

    async fn remove(
        &self,
        dirid: fileid3,
        filename: &filename3,
    ) -> std::result::Result<(), nfsstat3> {
        let n = name(filename)?;
        let a = self.vfs.lookup(dirid, &n).await.map_err(stat)?;
        if a.is_dir {
            self.vfs.rmdir(dirid, &n).await.map_err(stat)
        } else {
            self.vfs.unlink(dirid, &n).await.map_err(stat)
        }
    }

    async fn rename(
        &self,
        from_dirid: fileid3,
        from_filename: &filename3,
        to_dirid: fileid3,
        to_filename: &filename3,
    ) -> std::result::Result<(), nfsstat3> {
        let (from, to) = (name(from_filename)?, name(to_filename)?);
        // Replacing an existing file (an editor's safe save): remove it
        // first so the rename lands as the new current version.
        if let Ok(existing) = self.vfs.lookup(to_dirid, &to).await
            && !existing.is_dir
        {
            let _ = self.vfs.unlink(to_dirid, &to).await;
        }
        self.vfs
            .rename(from_dirid, &from, to_dirid, &to)
            .await
            .map_err(stat)
    }

    async fn readdir(
        &self,
        dirid: fileid3,
        start_after: fileid3,
        max_entries: usize,
    ) -> std::result::Result<ReadDirResult, nfsstat3> {
        let all = self.vfs.readdir(dirid).await.map_err(stat)?;
        let start = if start_after == 0 {
            0
        } else {
            match all.iter().position(|(_, a)| a.ino == start_after) {
                Some(i) => i + 1,
                None => return Err(nfsstat3::NFS3ERR_BAD_COOKIE),
            }
        };
        let mut entries = vec![];
        for (n, a) in all.iter().skip(start).take(max_entries) {
            entries.push(DirEntry {
                fileid: a.ino,
                name: n.as_bytes().into(),
                attr: self.fattr(a),
            });
        }
        let end = start + entries.len() >= all.len();
        Ok(ReadDirResult { entries, end })
    }

    async fn symlink(
        &self,
        _dirid: fileid3,
        _linkname: &filename3,
        _symlink: &nfspath3,
        _attr: &sattr3,
    ) -> std::result::Result<(fileid3, fattr3), nfsstat3> {
        Err(nfsstat3::NFS3ERR_NOTSUPP)
    }

    async fn readlink(&self, _id: fileid3) -> std::result::Result<nfspath3, nfsstat3> {
        Err(nfsstat3::NFS3ERR_NOTSUPP)
    }

    async fn fsinfo(&self, root_fileid: fileid3) -> std::result::Result<fsinfo3, nfsstat3> {
        let attr = self.getattr(root_fileid).await?;
        Ok(fsinfo3 {
            obj_attributes: nfsserve::nfs::post_op_attr::attributes(attr),
            rtmax: IO_SIZE,
            rtpref: IO_SIZE,
            rtmult: 4096,
            wtmax: IO_SIZE,
            wtpref: IO_SIZE,
            wtmult: 4096,
            dtpref: 64 * 1024,
            maxfilesize: 5 * 1024 * 1024 * 1024 * 1024,
            time_delta: nfstime3 {
                seconds: 0,
                nseconds: 1_000_000,
            },
            properties: nfsserve::nfs::FSF_HOMOGENEOUS | nfsserve::nfs::FSF_CANSETTIME,
        })
    }
}

fn parent_key(key: &str) -> String {
    let k = key.trim_end_matches('/');
    match k.rfind('/') {
        Some(i) => k[..=i].to_string(),
        None => String::new(),
    }
}

/// A running NFS server for one [`Vfs`].
pub struct NfsServer {
    port: u16,
    vfs: Arc<Vfs>,
    accept: tokio::task::JoinHandle<()>,
    uploader: tokio::task::JoinHandle<()>,
}

impl std::fmt::Debug for NfsServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NfsServer")
            .field("port", &self.port)
            .finish()
    }
}

impl NfsServer {
    /// Serves `vfs` on `127.0.0.1:<random port>`.
    pub async fn start(vfs: Arc<Vfs>) -> Result<NfsServer> {
        Self::start_with(vfs, false).await
    }

    /// Serves a Space's view for its guest. The guest's NFS client is
    /// not the host's user (Windows mounts anonymously, as uid -2), so
    /// writable entries carry the write bits for everyone: its client
    /// would refuse a write on its own otherwise. The access rules stay
    /// here, and refuse what the view may not write.
    pub async fn start_for_guest(vfs: Arc<Vfs>) -> Result<NfsServer> {
        Self::start_with(vfs, true).await
    }

    async fn start_with(vfs: Arc<Vfs>, shared: bool) -> Result<NfsServer> {
        #[cfg(unix)]
        // SAFETY: getuid/getgid never fail.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        #[cfg(not(unix))]
        let (uid, gid) = (0, 0);
        let fs = NfsFs {
            vfs: vfs.clone(),
            uid,
            gid,
            shared,
            generation: next_generation(),
        };
        let listener = NFSTcpListener::bind("127.0.0.1:0", fs)
            .await
            .map_err(|e| Error::Backend(format!("nfs listen: {e}")))?;
        let port = listener.get_listen_port();
        let accept = tokio::spawn(async move {
            let _ = listener.handle_forever().await;
        });
        let v = vfs.clone();
        let uploader = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                v.upload_idle().await;
            }
        });
        Ok(NfsServer {
            port,
            vfs,
            accept,
            uploader,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn vfs(&self) -> &Arc<Vfs> {
        &self.vfs
    }

    /// The `mount_nfs` options for this server.
    pub fn mount_options(&self) -> String {
        format!(
            "port={p},mountport={p},vers=3,tcp,nolocks,locallocks,rsize={IO_SIZE},wsize={IO_SIZE},readahead=16,actimeo=1,soft,timeo=100,retrans=3,intr,noowners",
            p = self.port
        )
    }

    /// Uploads what is pending and stops serving.
    pub async fn stop(self) -> Result<()> {
        self.uploader.abort();
        let r = self.vfs.flush_all().await;
        self.accept.abort();
        r
    }
}

impl Drop for NfsServer {
    fn drop(&mut self) {
        self.accept.abort();
        self.uploader.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Drive};

    fn fs(dir: &std::path::Path, ctx: Context, sub: &str) -> NfsFs {
        let drive = Drive::open_local(&dir.join("home"));
        NfsFs {
            vfs: Vfs::new(&drive, ctx, None, None, &dir.join(sub)).unwrap(),
            uid: 0,
            gid: 0,
            shared: true,
            generation: next_generation(),
        }
    }

    /// Two servers in one process (the user's view, then a Space's) never
    /// share a file handle: a guest's NFS client that keeps its caches
    /// across mounts (Windows does) would otherwise list the earlier view.
    #[tokio::test]
    async fn each_server_has_its_own_file_handles() {
        let dir = tempfile::tempdir().unwrap();
        let user = fs(dir.path(), Context::user(), "u");
        let space = fs(dir.path(), Context::space("local:lab"), "s");
        let root = user.root_dir();
        assert_eq!(root, space.root_dir());
        let (a, b) = (user.id_to_fh(root), space.id_to_fh(root));
        assert_ne!(a.data, b.data, "the same file, different handles");
        assert_ne!(user.serverid(), space.serverid());
        let code = |r: std::result::Result<fileid3, nfsstat3>| r.map_err(|e| e as u32);
        assert_eq!(code(user.fh_to_id(&a)), Ok(root));
        assert_eq!(code(space.fh_to_id(&b)), Ok(root));
        // The later server refuses the earlier one's handles as stale, and
        // the earlier one never takes a later one's.
        let (stale, bad) = (
            nfsstat3::NFS3ERR_STALE as u32,
            nfsstat3::NFS3ERR_BADHANDLE as u32,
        );
        assert_eq!(code(space.fh_to_id(&a)), Err(stale));
        assert_eq!(code(user.fh_to_id(&b)), Err(bad));
        assert_eq!(
            code(user.fh_to_id(&nfs_fh3 {
                data: vec![1, 2, 3]
            })),
            Err(bad)
        );
        // Any per-mount export name mounts the root; the rest of the path
        // resolves as usual.
        for p in ["/", "", "/cua-volume-vol-abc", "/cua-volume-x/", "/."] {
            assert_eq!(code(space.path_to_id(p.as_bytes()).await), Ok(root), "{p}");
        }
        let spaces = code(space.path_to_id(b"/cua-volume-vol-abc/spaces").await).unwrap();
        assert_eq!(code(space.path_to_id(b"/spaces").await), Ok(spaces));
        assert_ne!(spaces, root);
        assert_eq!(
            code(space.path_to_id(b"/cua-volume-x/agents").await),
            Err(nfsstat3::NFS3ERR_NOENT as u32),
            "a Space's view has no agents/"
        );
        // A rename within one directory, reached through a per-mount export
        // and through `/`.
        for (export, from, to) in [
            (
                "/cua-volume-0123abcd/spaces/local-lab",
                "out.txt",
                "done.txt",
            ),
            ("/spaces/local-lab", "done.txt", "final.txt"),
        ] {
            let dir = code(space.path_to_id(export.as_bytes()).await).unwrap();
            if from == "out.txt" {
                let (f, _) = space
                    .create(dir, &b"out.txt".to_vec().into(), sattr3::default())
                    .await
                    .map_err(|e| e as u32)
                    .unwrap();
                space
                    .write(f, 0, b"from the guest")
                    .await
                    .map_err(|e| e as u32)
                    .unwrap();
            }
            space
                .rename(
                    dir,
                    &from.as_bytes().to_vec().into(),
                    dir,
                    &to.as_bytes().to_vec().into(),
                )
                .await
                .map_err(|e| e as u32)
                .unwrap();
            assert!(
                space
                    .lookup(dir, &from.as_bytes().to_vec().into())
                    .await
                    .is_err()
            );
            let f = code(space.lookup(dir, &to.as_bytes().to_vec().into()).await).unwrap();
            assert_eq!(
                space.read(f, 0, 100).await.map_err(|e| e as u32).unwrap().0,
                b"from the guest"
            );
        }
        let fsid = |f: &NfsFs, a: &Attr| f.fattr(a).fsid;
        let attr = user.vfs.getattr(root).await.unwrap();
        assert_ne!(fsid(&user, &attr), fsid(&space, &attr));
        assert!(fsid(&user, &attr) < 1 << 31 && fsid(&space, &attr) < 1 << 31);
    }
}
