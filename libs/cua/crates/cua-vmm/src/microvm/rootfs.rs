//! Disk images of a microVM: the image's root filesystem as one read-only
//! squashfs (built once per image digest), the init's initramfs, and the
//! empty ext4 a VM's scratch disk is copied from.
//!
//! The rootfs is built without privileges: the OCI layers are merged into
//! one tar stream (whiteouts and opaque directories applied, ownership and
//! modes kept from the tar headers) and piped into `sqfstar`.

use std::collections::HashSet;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Result, VmmError};

/// Opens a layer blob, gunzipping it when it is gzip.
fn open_layer(path: &Path) -> Result<Box<dyn Read>> {
    let mut f = std::fs::File::open(path)?;
    let mut magic = [0u8; 4];
    let n = f.read(&mut magic)?;
    let f = BufReader::with_capacity(1 << 20, std::fs::File::open(path)?);
    if n >= 2 && magic[..2] == [0x1f, 0x8b] {
        return Ok(Box::new(flate2::read::GzDecoder::new(f)));
    }
    if n == 4 && magic == [0x28, 0xb5, 0x2f, 0xfd] {
        return Err(VmmError::invalid(format!(
            "{}: zstd-compressed layers are not supported for microVMs yet (use a gzip image)",
            path.display()
        )));
    }
    Ok(Box::new(f))
}

/// A layer path without `./` or a trailing `/`; `None` for the root and for
/// paths that leave it.
fn normalize(raw: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(raw);
    let mut parts = Vec::new();
    for c in s.split('/') {
        match c {
            "" | "." => {}
            ".." => return None,
            c => parts.push(c),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn parent_of(p: &str) -> &str {
    p.rsplit_once('/').map(|(a, _)| a).unwrap_or("")
}

/// Every proper ancestor of `p` (`a/b/c` → `a/b`, `a`).
fn ancestors(p: &str) -> impl Iterator<Item = &str> {
    let mut cur = p;
    std::iter::from_fn(move || {
        let (a, _) = cur.rsplit_once('/')?;
        cur = a;
        Some(a)
    })
}

/// Which entries of each layer (bottom first) end up in the merged tree.
///
/// Walks the layers top-down: an entry is kept unless an upper layer
/// already has its path, deleted it (`.wh.<name>`), made an ancestor
/// opaque (`.wh..wh..opq`) or replaced an ancestor with a non-directory.
pub fn plan_merge(layers: &[Vec<(String, bool)>]) -> Vec<Vec<bool>> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut nondir: HashSet<String> = HashSet::new();
    let mut deleted: HashSet<String> = HashSet::new();
    let mut opaque: HashSet<String> = HashSet::new();
    let mut keep: Vec<Vec<bool>> = layers.iter().map(|l| vec![false; l.len()]).collect();
    for (li, layer) in layers.iter().enumerate().rev() {
        let (mut new_deleted, mut new_opaque) = (Vec::new(), Vec::new());
        for (ei, (path, is_dir)) in layer.iter().enumerate() {
            let name = path.rsplit('/').next().unwrap_or(path);
            if name == ".wh..wh..opq" {
                new_opaque.push(parent_of(path).to_string());
                continue;
            }
            if let Some(target) = name.strip_prefix(".wh.") {
                let parent = parent_of(path);
                new_deleted.push(if parent.is_empty() {
                    target.to_string()
                } else {
                    format!("{parent}/{target}")
                });
                continue;
            }
            if seen.contains(path) || deleted.contains(path) {
                continue;
            }
            if ancestors(path)
                .any(|a| deleted.contains(a) || opaque.contains(a) || nondir.contains(a))
                || (!opaque.is_empty() && opaque.contains(""))
            {
                continue;
            }
            keep[li][ei] = true;
            seen.insert(path.clone());
            if !is_dir {
                nondir.insert(path.clone());
            }
        }
        deleted.extend(new_deleted);
        opaque.extend(new_opaque);
    }
    keep
}

/// Lists each layer's entries as `(path, is_dir)`, in archive order.
fn list_layer(path: &Path) -> Result<Vec<(String, bool)>> {
    let mut out = Vec::new();
    let mut a = tar::Archive::new(open_layer(path)?);
    for e in a.entries()? {
        let e = e?;
        let p = normalize(&e.path_bytes()).unwrap_or_default();
        out.push((p, e.header().entry_type().is_dir()));
    }
    Ok(out)
}

/// Writes the merged tree of `layers` (bottom first) as one tar to `out`.
pub fn merge_layers(layers: &[PathBuf], out: impl Write) -> Result<()> {
    let listed: Vec<Vec<(String, bool)>> = layers
        .iter()
        .map(|l| list_layer(l))
        .collect::<Result<_>>()?;
    let keep = plan_merge(&listed);
    let mut b = tar::Builder::new(out);
    b.mode(tar::HeaderMode::Complete);
    for (li, layer) in layers.iter().enumerate() {
        let mut a = tar::Archive::new(open_layer(layer)?);
        for (ei, e) in a.entries()?.enumerate() {
            let mut e = e?;
            if !keep[li].get(ei).copied().unwrap_or(false) {
                continue;
            }
            let Some(path) = normalize(&e.path_bytes()) else {
                continue;
            };
            let xattrs: Vec<(String, Vec<u8>)> = match e.pax_extensions() {
                Ok(Some(px)) => px
                    .flatten()
                    .filter_map(|x| {
                        let k = x.key().ok()?;
                        k.starts_with("SCHILY.xattr.")
                            .then(|| (k.to_string(), x.value_bytes().to_vec()))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            if !xattrs.is_empty() {
                b.append_pax_extensions(xattrs.iter().map(|(k, v)| (k.as_str(), v.as_slice())))?;
            }
            let mut h = e.header().clone();
            match h.entry_type() {
                tar::EntryType::Link | tar::EntryType::Symlink => {
                    let target = e
                        .link_name_bytes()
                        .map(|t| String::from_utf8_lossy(&t).into_owned())
                        .unwrap_or_default();
                    let target = if h.entry_type() == tar::EntryType::Link {
                        normalize(target.as_bytes()).unwrap_or(target)
                    } else {
                        target
                    };
                    b.append_link(&mut h, &path, &target)?;
                }
                _ => b.append_data(&mut h, &path, &mut e)?,
            }
        }
    }
    b.finish()?;
    Ok(())
}

/// Builds `out` (squashfs) from the image's layers with `sqfstar`.
pub fn build_squashfs(layers: &[PathBuf], out: &Path) -> Result<()> {
    let sqfstar = crate::host::which("sqfstar").ok_or_else(|| {
        VmmError::missing(
            "sqfstar",
            "install squashfs-tools 4.6 or newer (Ubuntu: sudo apt-get install squashfs-tools)",
        )
    })?;
    let partial = out.with_extension("partial");
    let _ = std::fs::remove_file(&partial);
    let mut child = Command::new(sqfstar)
        .args([
            "-quiet",
            "-no-progress",
            "-comp",
            "zstd",
            "-b",
            "1M",
            "-root-uid",
            "0",
            "-root-gid",
            "0",
            "-root-mode",
            "0755",
        ])
        .arg(&partial)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let merged = merge_layers(layers, std::io::BufWriter::with_capacity(1 << 20, stdin));
    let out_status = child.wait_with_output()?;
    if let Err(e) = merged {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    if !out_status.status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(VmmError::Command {
            cmd: "sqfstar".into(),
            code: out_status.status.code(),
            stderr: String::from_utf8_lossy(&out_status.stderr)
                .trim()
                .to_string(),
        });
    }
    std::fs::rename(&partial, out)?;
    Ok(())
}

/// A slice of the merged tarball, opened only while backhand reads it (a
/// rootfs has tens of thousands of files: one descriptor each would run
/// out).
struct TarSlice {
    path: std::sync::Arc<PathBuf>,
    offset: u64,
    left: u64,
    file: Option<std::io::Take<std::io::BufReader<std::fs::File>>>,
}

impl Read for TarSlice {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::Seek;
        if self.left == 0 {
            self.file = None;
            return Ok(0);
        }
        if self.file.is_none() {
            let mut f = std::fs::File::open(self.path.as_ref())?;
            f.seek(std::io::SeekFrom::Start(self.offset))?;
            self.file = Some(BufReader::with_capacity(1 << 20, f).take(self.left));
        }
        let n = self.file.as_mut().map_or(Ok(0), |f| f.read(buf))?;
        self.left -= n as u64;
        if n == 0 || self.left == 0 {
            self.file = None;
        }
        Ok(n)
    }
}

/// An extended attribute of one file in a rootfs built without `sqfstar`
/// (squashfs written in-process carries none): the guest init sets them at
/// boot from [`cua_microvm_init::XATTRS_FILE`] at the image's root.
#[derive(serde::Serialize)]
struct Xattr<'a> {
    path: &'a str,
    name: &'a str,
    value: String,
}

/// Squashfs's device number of `major:minor` (the kernel's old_encode_dev
/// layout extended to 32 bits, as mksquashfs writes it).
pub fn squashfs_dev(major: u32, minor: u32) -> u32 {
    ((major & 0xfff) << 8) | (minor & 0xff) | ((minor & !0xff) << 12)
}

/// Builds `out` (zstd squashfs, 1 MiB blocks) from the image's layers in
/// process, for hosts without `sqfstar` (macOS): the merged tree is written
/// to a tarball next to `out`, indexed, then added to the image in path
/// order (each file read from its slice of the tarball). Hard links become
/// copies (which squashfs deduplicates); extended attributes go to
/// [`cua_microvm_init::XATTRS_FILE`] for the guest init.
pub fn build_squashfs_native(layers: &[PathBuf], out: &Path) -> Result<()> {
    use backhand::compression::Compressor;
    use backhand::{FilesystemCompressor, FilesystemWriter, NodeHeader};
    use std::collections::{BTreeMap, HashMap};

    enum Kind {
        Dir,
        File { offset: u64, len: u64 },
        Link(String),
        Symlink(String),
        Char(u32),
        Block(u32),
        Fifo,
    }
    let tarball = out.with_extension("tar.partial");
    let partial = out.with_extension("partial");
    let _ = std::fs::remove_file(&partial);
    let result = (|| -> Result<()> {
        merge_layers(
            layers,
            std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tarball)?),
        )?;
        // Path → what it is, its header, and its xattrs. Sorted by path
        // components (backhand's own order), so every node is appended.
        let mut entries: BTreeMap<PathBuf, (Kind, NodeHeader)> = BTreeMap::new();
        let mut xattrs: Vec<(String, String, Vec<u8>)> = Vec::new();
        let mut a = tar::Archive::new(BufReader::with_capacity(
            1 << 20,
            std::fs::File::open(&tarball)?,
        ));
        for e in a.entries()? {
            let mut e = e?;
            let Some(path) = normalize(&e.path_bytes()) else {
                continue;
            };
            if let Ok(Some(px)) = e.pax_extensions() {
                for x in px.flatten() {
                    if let Ok(k) = x.key()
                        && let Some(name) = k.strip_prefix("SCHILY.xattr.")
                    {
                        xattrs.push((path.clone(), name.to_string(), x.value_bytes().to_vec()));
                    }
                }
            }
            let h = e.header();
            let header = NodeHeader::new(
                (h.mode().unwrap_or(0o644) & 0o7777) as u16,
                h.uid().unwrap_or(0) as u32,
                h.gid().unwrap_or(0) as u32,
                h.mtime().unwrap_or(0).min(u64::from(u32::MAX)) as u32,
            );
            let dev = || {
                squashfs_dev(
                    h.device_major().ok().flatten().unwrap_or(0),
                    h.device_minor().ok().flatten().unwrap_or(0),
                )
            };
            let link = || {
                e.link_name_bytes()
                    .map(|t| String::from_utf8_lossy(&t).into_owned())
                    .unwrap_or_default()
            };
            let kind = match h.entry_type() {
                tar::EntryType::Directory => Kind::Dir,
                tar::EntryType::Symlink => Kind::Symlink(link()),
                tar::EntryType::Link => {
                    Kind::Link(normalize(link().as_bytes()).unwrap_or_default())
                }
                tar::EntryType::Char => Kind::Char(dev()),
                tar::EntryType::Block => Kind::Block(dev()),
                tar::EntryType::Fifo => Kind::Fifo,
                tar::EntryType::Regular
                | tar::EntryType::Continuous
                | tar::EntryType::GNUSparse => Kind::File {
                    offset: e.raw_file_position(),
                    len: e.size(),
                },
                _ => continue,
            };
            entries.insert(PathBuf::from("/").join(&path), (kind, header));
        }
        let files: HashMap<PathBuf, (u64, u64, NodeHeader)> = entries
            .iter()
            .filter_map(|(p, (k, h))| match k {
                Kind::File { offset, len } => Some((p.clone(), (*offset, *len, *h))),
                _ => None,
            })
            .collect();
        let shared = std::sync::Arc::new(tarball.clone());
        let slice = |offset: u64, len: u64| TarSlice {
            path: shared.clone(),
            offset,
            left: len,
            file: None,
        };
        let mut fs = FilesystemWriter::default();
        fs.set_compressor(
            FilesystemCompressor::new(Compressor::Zstd, None)
                .map_err(|e| VmmError::other(format!("squashfs: {e}")))?,
        );
        fs.set_block_size(1 << 20);
        fs.set_time(0);
        fs.set_root_mode(0o755);
        fs.set_root_uid(0);
        fs.set_root_gid(0);
        let bad = |p: &Path, e: backhand::BackhandError| {
            VmmError::other(format!("squashfs: {}: {e}", p.display()))
        };
        // Parents an image leaves out (layers need not list them).
        let mut dirs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        dirs.insert(PathBuf::from("/"));
        let implicit = NodeHeader::new(0o755, 0, 0, 0);
        for (path, (kind, header)) in &entries {
            if let Some(parent) = path.parent()
                && !dirs.contains(parent)
            {
                fs.push_dir_all(parent, implicit)
                    .map_err(|e| bad(parent, e))?;
                for a in parent.ancestors() {
                    dirs.insert(a.to_path_buf());
                }
            }
            let r = match kind {
                Kind::Dir => {
                    dirs.insert(path.clone());
                    fs.push_dir(path, *header)
                }
                Kind::File { offset, len } => fs.push_file(slice(*offset, *len), path, *header),
                Kind::Link(target) => match files.get(&PathBuf::from("/").join(target)) {
                    // A hard link keeps its target's metadata.
                    Some((offset, len, h)) => fs.push_file(slice(*offset, *len), path, *h),
                    None => continue,
                },
                Kind::Symlink(target) => fs.push_symlink(target, path, *header),
                Kind::Char(d) => fs.push_char_device(*d, path, *header),
                Kind::Block(d) => fs.push_block_device(*d, path, *header),
                Kind::Fifo => fs.push_fifo(path, *header),
            };
            r.map_err(|e| bad(path, e))?;
        }
        let listed: Vec<Xattr> = xattrs
            .iter()
            .map(|(path, name, value)| Xattr {
                path,
                name,
                value: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, value),
            })
            .collect();
        let sidecar = serde_json::to_vec(&listed)?;
        if !listed.is_empty() {
            fs.push_file(
                std::io::Cursor::new(sidecar),
                format!("/{}", cua_microvm_init::XATTRS_FILE),
                NodeHeader::new(0o600, 0, 0, 0),
            )
            .map_err(|e| bad(Path::new(cua_microvm_init::XATTRS_FILE), e))?;
        }
        let mut w = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&partial)?);
        fs.write(&mut w)
            .map_err(|e| VmmError::other(format!("squashfs: {e}")))?;
        w.flush()?;
        drop(w);
        std::fs::rename(&partial, out)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&tarball);
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

#[cfg(not(unix))]
pub fn expand_template(_template: &[u8], _dest: &Path, _size: u64) -> Result<()> {
    Err(super::unix_only("a scratch disk"))
}

/// The empty ext4 a scratch disk starts from where `mkfs.ext4` is missing
/// (macOS): 1 GiB, grown by the guest init to fill the disk file
/// (scripts/make-scratch-template.sh makes it).
pub const SCRATCH_TEMPLATE: &[u8] = include_bytes!("scratch-1g.ext4.sparse");

/// Writes the [`SCRATCH_TEMPLATE`]-format image `template` to `dest` as a
/// sparse file of `size` bytes (at least the image's own size).
#[cfg(unix)]
pub fn expand_template(template: &[u8], dest: &Path, size: u64) -> Result<()> {
    use std::os::unix::fs::FileExt;
    const MAGIC: &[u8] = b"CUASPARSE1";
    let bad = || VmmError::other("the scratch disk template is corrupt");
    let rest = template.strip_prefix(MAGIC).ok_or_else(bad)?;
    let image = u64::from_le_bytes(
        rest.get(..8)
            .ok_or_else(bad)?
            .try_into()
            .map_err(|_| bad())?,
    );
    let mut body = Vec::new();
    flate2::read::ZlibDecoder::new(&rest[8..]).read_to_end(&mut body)?;
    if body.len() % (8 + 4096) != 0 {
        return Err(bad());
    }
    let partial = dest.with_extension("partial");
    let _ = std::fs::remove_file(&partial);
    let f = std::fs::File::create(&partial)?;
    f.set_len(size.max(image))?;
    for rec in body.chunks_exact(8 + 4096) {
        let off = u64::from_le_bytes(rec[..8].try_into().map_err(|_| bad())?);
        if off + 4096 > image {
            let _ = std::fs::remove_file(&partial);
            return Err(bad());
        }
        f.write_all_at(&rec[8..], off)?;
    }
    f.sync_all()?;
    drop(f);
    std::fs::rename(&partial, dest)?;
    Ok(())
}

/// One entry's metadata, applied to the ext4 image after `mkfs.ext4 -d`
/// (which copies the unprivileged extraction's owner and modes).
struct Meta {
    path: String,
    uid: u64,
    gid: u64,
    /// Full `st_mode` (type and permission bits).
    mode: u32,
    /// Character or block device: (`c`/`b`, major, minor).
    dev: Option<(char, u32, u32)>,
}

fn debugfs_quote(p: &str) -> String {
    format!("\"/{}\"", p.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The debugfs script that gives every entry its owner and mode, and
/// creates device nodes (an unprivileged extraction cannot).
fn debugfs_script(metas: &[Meta]) -> String {
    let mut out = String::new();
    for m in metas {
        let q = debugfs_quote(&m.path);
        if let Some((t, maj, min)) = m.dev {
            out.push_str(&format!("mknod {q} {t} {maj} {min}\n"));
        }
        out.push_str(&format!("sif {q} uid {}\n", m.uid));
        out.push_str(&format!("sif {q} gid {}\n", m.gid));
        out.push_str(&format!("sif {q} mode 0{:o}\n", m.mode));
    }
    out
}

/// Builds `out`, an ext4 image of the image's layers sized for read-only
/// use, without privileges: the merged tree is extracted as this user,
/// copied in by `mkfs.ext4 -d`, then `debugfs` restores every owner and
/// mode and creates device nodes. An ext4 base can be mapped into guests
/// with DAX (virtio-pmem): every VM reads the host's page cache, so a
/// browser's binary is in memory once per host, not once per VM.
pub fn build_ext4(layers: &[PathBuf], out: &Path) -> Result<()> {
    let mkfs = crate::host::which("mkfs.ext4").ok_or_else(|| {
        VmmError::missing(
            "mkfs.ext4",
            "install e2fsprogs (sudo apt-get install e2fsprogs)",
        )
    })?;
    let debugfs = crate::host::which("debugfs").ok_or_else(|| {
        VmmError::missing(
            "debugfs",
            "install e2fsprogs (sudo apt-get install e2fsprogs)",
        )
    })?;
    let work = out.with_extension("work");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let result = (|| -> Result<()> {
        let tarball = work.join("rootfs.tar");
        merge_layers(
            layers,
            std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&tarball)?),
        )?;
        let root = work.join("root");
        std::fs::create_dir_all(&root)?;
        let mut metas = Vec::new();
        let mut a = tar::Archive::new(BufReader::with_capacity(
            1 << 20,
            std::fs::File::open(&tarball)?,
        ));
        a.set_preserve_permissions(false);
        a.set_unpack_xattrs(false);
        a.set_overwrite(true);
        for e in a.entries()? {
            let mut e = e?;
            let Some(path) = normalize(&e.path_bytes()) else {
                continue;
            };
            let h = e.header();
            let kind = h.entry_type();
            let perm = h.mode().unwrap_or(0o644) & 0o7777;
            let (ty, dev) = match kind {
                tar::EntryType::Directory => (0o040000, None),
                tar::EntryType::Symlink => (0o120000, None),
                tar::EntryType::Char => (
                    0o020000,
                    Some((
                        'c',
                        h.device_major().ok().flatten().unwrap_or(0),
                        h.device_minor().ok().flatten().unwrap_or(0),
                    )),
                ),
                tar::EntryType::Block => (
                    0o060000,
                    Some((
                        'b',
                        h.device_major().ok().flatten().unwrap_or(0),
                        h.device_minor().ok().flatten().unwrap_or(0),
                    )),
                ),
                tar::EntryType::Fifo => (0o010000, None),
                _ => (0o100000, None),
            };
            let meta = Meta {
                path: path.clone(),
                uid: h.uid().unwrap_or(0),
                gid: h.gid().unwrap_or(0),
                mode: ty | perm,
                dev,
            };
            match kind {
                // Created in the image by debugfs.
                tar::EntryType::Char | tar::EntryType::Block => {}
                tar::EntryType::Fifo => {
                    let p = root.join(&path);
                    if let Some(parent) = p.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes())
                        .map_err(|e| VmmError::other(e.to_string()))?;
                    #[cfg(unix)]
                    unsafe {
                        libc::mkfifo(c.as_ptr(), 0o644);
                    }
                    #[cfg(not(unix))]
                    let _ = c;
                }
                tar::EntryType::Link => {
                    // Hard links keep the target's metadata.
                    e.unpack_in(&root)?;
                    continue;
                }
                _ => {
                    e.unpack_in(&root)?;
                }
            }
            #[cfg(unix)]
            if kind != tar::EntryType::Symlink {
                // Owner-writable while mkfs copies the tree (debugfs puts
                // the real mode back).
                let p = root.join(&path);
                if let Ok(md) = std::fs::symlink_metadata(&p) {
                    use std::os::unix::fs::PermissionsExt;
                    let want = if md.is_dir() { 0o755 } else { 0o644 };
                    let _ = std::fs::set_permissions(
                        &p,
                        std::fs::Permissions::from_mode(md.permissions().mode() | want),
                    );
                }
            }
            metas.push(meta);
        }
        std::fs::remove_file(&tarball)?;
        let used = crate::disk::allocated_size(&root);
        // Room for metadata and the device nodes; 2 MiB aligned (pmem).
        let size = (used + used / 5 + (256 << 20)).div_ceil(2 << 20) * (2 << 20);
        let partial = out.with_extension("partial");
        let _ = std::fs::remove_file(&partial);
        std::fs::File::create(&partial)?.set_len(size)?;
        let o = Command::new(&mkfs)
            .args([
                "-q",
                "-F",
                "-m",
                "0",
                "-b",
                "4096",
                "-O",
                "^has_journal",
                "-E",
                "root_owner=0:0",
                "-d",
            ])
            .arg(&root)
            .arg(&partial)
            .stdin(Stdio::null())
            .output()?;
        if !o.status.success() {
            let _ = std::fs::remove_file(&partial);
            return Err(VmmError::Command {
                cmd: "mkfs.ext4 -d".into(),
                code: o.status.code(),
                stderr: String::from_utf8_lossy(&o.stderr).trim().to_string(),
            });
        }
        let script = work.join("fixups.debugfs");
        std::fs::write(&script, debugfs_script(&metas))?;
        let o = Command::new(&debugfs)
            .arg("-w")
            .arg("-f")
            .arg(&script)
            .arg(&partial)
            .stdin(Stdio::null())
            .output()?;
        if !o.status.success() {
            let _ = std::fs::remove_file(&partial);
            return Err(VmmError::Command {
                cmd: "debugfs".into(),
                code: o.status.code(),
                stderr: String::from_utf8_lossy(&o.stderr).trim().to_string(),
            });
        }
        std::fs::rename(&partial, out)?;
        Ok(())
    })();
    // Directories the extraction made unwritable would block removal.
    let _ = Command::new("chmod")
        .args(["-R", "u+rwX"])
        .arg(&work)
        .status();
    let _ = std::fs::remove_dir_all(&work);
    result
}

pub use crate::elf::interpreter as elf_interpreter;

/// One `newc` cpio entry.
fn cpio_entry(out: &mut Vec<u8>, ino: u32, name: &str, mode: u32, rdev: (u32, u32), data: &[u8]) {
    let namesize = name.len() + 1;
    let hdr = format!(
        "070701{ino:08X}{mode:08X}{uid:08X}{gid:08X}{nlink:08X}{mtime:08X}{size:08X}{dmaj:08X}{dmin:08X}{rmaj:08X}{rmin:08X}{namesize:08X}{check:08X}",
        uid = 0,
        gid = 0,
        nlink = if mode & 0o040000 != 0 { 2 } else { 1 },
        mtime = 0,
        size = data.len(),
        dmaj = 0,
        dmin = 0,
        rmaj = rdev.0,
        rmin = rdev.1,
        check = 0,
    );
    out.extend_from_slice(hdr.as_bytes());
    out.extend_from_slice(name.as_bytes());
    out.push(0);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    out.extend_from_slice(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

/// The initramfs: the init as `/init`, its mount points and
/// `/dev/console` (the kernel opens it for PID 1's output).
pub fn initramfs(init: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(init.len() + 4096);
    let mut ino = 1;
    for d in ["dev", "proc", "sys", "base", "rw", "newroot"] {
        cpio_entry(&mut out, ino, d, 0o040755, (0, 0), &[]);
        ino += 1;
    }
    cpio_entry(&mut out, ino, "dev/console", 0o020600, (5, 1), &[]);
    ino += 1;
    cpio_entry(&mut out, ino, "init", 0o100755, (0, 0), init);
    cpio_entry(&mut out, 0, "TRAILER!!!", 0, (0, 0), &[]);
    while !out.len().is_multiple_of(512) {
        out.push(0);
    }
    out
}

/// An empty, sparse ext4 of `gib` GiB (the scratch disk template).
pub fn make_ext4(path: &Path, gib: u32) -> Result<()> {
    let mkfs = crate::host::which("mkfs.ext4").ok_or_else(|| {
        VmmError::missing(
            "mkfs.ext4",
            "install e2fsprogs (sudo apt-get install e2fsprogs)",
        )
    })?;
    let partial = path.with_extension("partial");
    let _ = std::fs::remove_file(&partial);
    let f = std::fs::File::create(&partial)?;
    f.set_len(u64::from(gib) << 30)?;
    drop(f);
    let out = Command::new(mkfs)
        .args([
            "-q",
            "-F",
            "-m",
            "0",
            "-E",
            "lazy_itable_init=1,lazy_journal_init=1",
        ])
        .arg(&partial)
        .stdin(Stdio::null())
        .output()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&partial);
        return Err(VmmError::Command {
            cmd: "mkfs.ext4".into(),
            code: out.status.code(),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }
    std::fs::rename(&partial, path)?;
    Ok(())
}

/// Copies a (sparse) disk file: a reflink where the filesystem has them,
/// else a sparse copy. Holes stay holes either way. On macOS an APFS
/// clone (`clonefile`): instant, and blocks are shared until written.
pub async fn copy_sparse(from: &Path, to: &Path) -> Result<()> {
    let partial = to.with_extension("partial");
    let _ = tokio::fs::remove_file(&partial).await;
    #[cfg(target_os = "macos")]
    {
        let (src, dst) = (from.to_path_buf(), partial.clone());
        tokio::task::spawn_blocking(move || clone_file(&src, &dst))
            .await
            .map_err(|e| VmmError::other(e.to_string()))??;
        tokio::fs::rename(&partial, to).await?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let out = tokio::process::Command::new("cp")
            .args(["--reflink=auto", "--sparse=always"])
            .arg(from)
            .arg(&partial)
            .stdin(Stdio::null())
            .output()
            .await?;
        if !out.status.success() {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(VmmError::Command {
                cmd: format!("cp {}", from.display()),
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
            });
        }
        tokio::fs::rename(&partial, to).await?;
        Ok(())
    }
}

/// `clonefile(2)` of `from` to `to` (which must not exist); a plain copy
/// when the volume cannot clone (not APFS, or another volume).
#[cfg(target_os = "macos")]
pub fn clone_file(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| {
        std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|e| VmmError::other(e.to_string()))
    };
    let (src, dst) = (c(from)?, c(to)?);
    // SAFETY: two valid C strings; no flags.
    if unsafe { libc::clonefile(src.as_ptr(), dst.as_ptr(), 0) } == 0 {
        return Ok(());
    }
    let e = std::io::Error::last_os_error();
    match e.raw_os_error() {
        Some(libc::ENOTSUP) | Some(libc::EXDEV) => {
            std::fs::copy(from, to)?;
            Ok(())
        }
        _ => Err(VmmError::other(format!(
            "clone {} to {}: {e}",
            from.display(),
            to.display()
        ))),
    }
}

/// Writes every data extent of sparse file `from` into `to` at the same
/// offsets (a diff snapshot's changed pages over a copy of its base).
/// Returns the bytes written.
#[cfg(not(unix))]
pub fn overlay_data(from: &Path, to: &Path) -> Result<u64> {
    let _ = (from, to);
    Err(super::unix_only("overlay_data"))
}

/// Writes every data extent of sparse file `from` into `to` at the same
/// offsets (a diff snapshot's changed pages over a copy of its base).
/// Returns the bytes written.
#[cfg(unix)]
pub fn overlay_data(from: &Path, to: &Path) -> Result<u64> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::FileExt;
    let src = std::fs::File::open(from)?;
    let dst = std::fs::OpenOptions::new().write(true).open(to)?;
    let len = src.metadata()?.len();
    if dst.metadata()?.len() < len {
        dst.set_len(len)?;
    }
    let fd = src.as_raw_fd();
    let mut buf = vec![0u8; 1 << 20];
    let (mut off, mut written) = (0u64, 0u64);
    while off < len {
        // SAFETY: lseek on an open descriptor; ENXIO means no data past `off`.
        let data = unsafe { libc::lseek(fd, off as libc::off_t, libc::SEEK_DATA) };
        if data < 0 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::ENXIO) {
                break;
            }
            return Err(e.into());
        }
        let hole = unsafe { libc::lseek(fd, data, libc::SEEK_HOLE) };
        if hole < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let (mut at, end) = (data as u64, (hole as u64).min(len));
        while at < end {
            let n = ((end - at) as usize).min(buf.len());
            src.read_exact_at(&mut buf[..n], at)?;
            dst.write_all_at(&buf[..n], at)?;
            at += n as u64;
            written += n as u64;
        }
        off = end;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(entries: &[(&str, bool)]) -> Vec<(String, bool)> {
        entries.iter().map(|(p, d)| (p.to_string(), *d)).collect()
    }

    // Firecracker diff snapshots (overlay_data's only caller) exist on Linux
    // hosts only. APFS reports SEEK_DATA/SEEK_HOLE at its own allocation
    // granularity, so on macOS the holes of this sparse file read back as data.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_diff_lands_on_its_base_at_its_offsets() {
        use std::os::unix::fs::FileExt;
        let d = tempfile::tempdir().unwrap();
        let (base, diff) = (d.path().join("base"), d.path().join("diff"));
        let page = 4096u64;
        std::fs::write(&base, vec![1u8; (8 * page) as usize]).unwrap();
        // A sparse diff: pages 2 and 6 changed.
        let f = std::fs::File::create(&diff).unwrap();
        f.set_len(8 * page).unwrap();
        f.write_all_at(&vec![9u8; page as usize], 2 * page).unwrap();
        f.write_all_at(&vec![7u8; page as usize], 6 * page).unwrap();
        drop(f);
        let n = overlay_data(&diff, &base).unwrap();
        let out = std::fs::read(&base).unwrap();
        assert_eq!(out.len() as u64, 8 * page);
        for (i, chunk) in out.chunks(page as usize).enumerate() {
            let want = match i {
                2 => 9,
                6 => 7,
                _ => 1,
            };
            assert!(chunk.iter().all(|b| *b == want), "page {i}");
        }
        // Only the data extents were copied (filesystems may round them
        // up, never down).
        assert!((2 * page..=8 * page).contains(&n), "{n}");
    }

    #[test]
    fn upper_layers_win_and_whiteouts_delete() {
        let l0 = layer(&[
            ("etc", true),
            ("etc/a", false),
            ("etc/b", false),
            ("opt", true),
            ("opt/x", false),
            ("var", true),
            ("var/log", true),
            ("var/log/old", false),
            ("link", false),
        ]);
        let l1 = layer(&[
            ("etc", true),
            ("etc/a", false),
            ("etc/.wh.b", false),
            ("opt/.wh..wh..opq", false),
            ("opt/y", false),
            ("var/.wh.log", false),
            ("link", true),
            ("link/inside", false),
        ]);
        let keep = plan_merge(&[l0.clone(), l1.clone()]);
        let kept = |li: usize, l: &[(String, bool)]| -> Vec<String> {
            l.iter()
                .zip(&keep[li])
                .filter(|(_, k)| **k)
                .map(|((p, _), _)| p.clone())
                .collect()
        };
        assert_eq!(
            kept(1, &l1),
            ["etc", "etc/a", "opt/y", "link", "link/inside"]
        );
        // etc/a and etc come from the upper layer; b is deleted, the opaque
        // opt hides x, the deleted var/log hides its file, and the upper
        // `link` directory replaces the lower file.
        assert_eq!(kept(0, &l0), ["opt", "var"]);
    }

    #[test]
    fn a_non_directory_hides_what_was_below_it() {
        let l0 = layer(&[("usr", true), ("usr/lib", true), ("usr/lib/x", false)]);
        let l1 = layer(&[("usr/lib", false)]);
        let keep = plan_merge(&[l0, l1]);
        assert_eq!(keep[0], [true, false, false]);
        assert_eq!(keep[1], [true]);
    }

    #[test]
    fn paths_are_normalized() {
        assert_eq!(normalize(b"./etc/passwd").as_deref(), Some("etc/passwd"));
        assert_eq!(normalize(b"usr/bin/").as_deref(), Some("usr/bin"));
        assert_eq!(normalize(b"./"), None);
        assert_eq!(normalize(b"../escape"), None);
        assert_eq!(ancestors("a/b/c").collect::<Vec<_>>(), ["a/b", "a"]);
    }

    #[test]
    fn merged_tar_keeps_ownership_and_links() {
        let dir = tempfile::tempdir().unwrap();
        let mk = |name: &str, f: &dyn Fn(&mut tar::Builder<Vec<u8>>)| {
            let mut b = tar::Builder::new(Vec::new());
            f(&mut b);
            let p = dir.path().join(name);
            std::fs::write(&p, b.into_inner().unwrap()).unwrap();
            p
        };
        let file = |b: &mut tar::Builder<Vec<u8>>, path: &str, data: &[u8], uid: u64| {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Regular);
            h.set_size(data.len() as u64);
            h.set_mode(0o4755);
            h.set_uid(uid);
            b.append_data(&mut h, path, data).unwrap();
        };
        let l0 = mk("l0.tar", &|b| {
            file(b, "bin/su", b"su", 0);
            file(b, "home/cua/.profile", b"p", 1000);
            file(b, "gone", b"x", 0);
        });
        let l1 = mk("l1.tar", &|b| {
            file(b, ".wh.gone", b"", 0);
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Link);
            h.set_size(0);
            b.append_link(&mut h, "bin/su2", "bin/su").unwrap();
        });
        let mut out = Vec::new();
        merge_layers(&[l0, l1], &mut out).unwrap();
        let mut a = tar::Archive::new(&out[..]);
        let mut seen = Vec::new();
        for e in a.entries().unwrap() {
            let e = e.unwrap();
            let p = e.path().unwrap().display().to_string();
            match p.as_str() {
                "bin/su" => assert_eq!(e.header().mode().unwrap(), 0o4755),
                "home/cua/.profile" => assert_eq!(e.header().uid().unwrap(), 1000),
                "bin/su2" => assert_eq!(
                    e.link_name().unwrap().unwrap().display().to_string(),
                    "bin/su"
                ),
                other => panic!("unexpected {other}"),
            }
            seen.push(p);
        }
        assert_eq!(seen.len(), 3, "{seen:?}");
    }

    /// The in-process squashfs (macOS) holds the merged tree as `sqfstar`
    /// would: owners, modes, links, devices, parents a layer leaves out, and
    /// the extended attributes listed for the guest init.
    #[test]
    fn the_native_squashfs_holds_the_merged_tree() {
        use backhand::{FilesystemReader, InnerNode};
        let dir = tempfile::tempdir().unwrap();
        let mut b = tar::Builder::new(Vec::new());
        let file = |b: &mut tar::Builder<Vec<u8>>, path: &str, data: &[u8], mode: u32, uid: u64| {
            let mut h = tar::Header::new_gnu();
            h.set_entry_type(tar::EntryType::Regular);
            h.set_size(data.len() as u64);
            h.set_mode(mode);
            h.set_uid(uid);
            h.set_gid(uid);
            h.set_mtime(1_700_000_000);
            b.append_data(&mut h, path, data).unwrap();
        };
        let mut d = tar::Header::new_gnu();
        d.set_entry_type(tar::EntryType::Directory);
        d.set_mode(0o750);
        d.set_uid(1000);
        d.set_size(0);
        b.append_data(&mut d, "home/cua/", &[][..]).unwrap();
        file(&mut b, "home/cua/.profile", b"profile", 0o644, 1000);
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        file(&mut b, "usr/bin/big", &big, 0o755, 0);
        // A capability, as a PAX header (the way image layers carry them).
        b.append_pax_extensions([("SCHILY.xattr.security.capability", &b"\x01\x00\x00\x02"[..])])
            .unwrap();
        file(&mut b, "usr/bin/ping", b"ping", 0o4755, 0);
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Link);
        h.set_size(0);
        b.append_link(&mut h, "usr/bin/pong", "usr/bin/ping")
            .unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        b.append_link(&mut h, "bin", "usr/bin").unwrap();
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Char);
        h.set_size(0);
        h.set_mode(0o666);
        h.set_device_major(1).unwrap();
        h.set_device_minor(3).unwrap();
        b.append_data(&mut h, "dev/null", &[][..]).unwrap();
        let layer = dir.path().join("l0.tar");
        std::fs::write(&layer, b.into_inner().unwrap()).unwrap();
        let out = dir.path().join("rootfs.sqfs");
        build_squashfs_native(&[layer], &out).unwrap();
        assert!(
            !out.with_extension("tar.partial").exists(),
            "the tarball is removed"
        );

        let fs = FilesystemReader::from_reader(std::io::BufReader::new(
            std::fs::File::open(&out).unwrap(),
        ))
        .unwrap();
        let node = |p: &str| {
            fs.files()
                .find(|n| n.fullpath == Path::new(p))
                .unwrap_or_else(|| panic!("{p} is missing"))
        };
        let read = |p: &str| -> Vec<u8> {
            let InnerNode::File(f) = &node(p).inner else {
                panic!("{p} is not a file")
            };
            let mut v = Vec::new();
            fs.file(f).reader().read_to_end(&mut v).unwrap();
            v
        };
        assert_eq!(read("/home/cua/.profile"), b"profile");
        assert_eq!(read("/usr/bin/big"), big, "a multi-block file");
        assert_eq!(read("/usr/bin/pong"), b"ping", "a hard link is a copy");
        let cua = node("/home/cua");
        assert_eq!((cua.header.permissions, cua.header.uid), (0o750, 1000));
        let ping = node("/usr/bin/ping");
        assert_eq!(ping.header.permissions, 0o4755, "setuid kept");
        assert_eq!(node("/usr").header.permissions, 0o755, "an implicit parent");
        match &node("/bin").inner {
            InnerNode::Symlink(l) => assert_eq!(l.link, Path::new("usr/bin")),
            _ => panic!("/bin is not a symlink"),
        }
        match &node("/dev/null").inner {
            InnerNode::CharacterDevice(c) => assert_eq!(c.device_number, squashfs_dev(1, 3)),
            _ => panic!("/dev/null is not a device"),
        }
        let listed: Vec<cua_microvm_init::Xattr> =
            serde_json::from_slice(&read(&format!("/{}", cua_microvm_init::XATTRS_FILE))).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, "usr/bin/ping");
        assert_eq!(listed[0].name, "security.capability");
        assert_eq!(listed[0].value, "AQAAAg==");
        assert_eq!(squashfs_dev(1, 3), 0x103);
        assert_eq!(
            squashfs_dev(259, 300),
            (259 << 8) | (300 & 0xff) | ((300 & !0xff) << 12)
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_scratch_template_expands_to_a_sparse_ext4_of_the_asked_size() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("scratch.ext4");
        expand_template(SCRATCH_TEMPLATE, &disk, 16 << 30).unwrap();
        let md = std::fs::metadata(&disk).unwrap();
        assert_eq!(md.len(), 16 << 30);
        assert!(
            crate::disk::allocated_size(&disk) < 64 << 20,
            "sparse: {} bytes allocated",
            crate::disk::allocated_size(&disk)
        );
        let mut sb = vec![0u8; 1024];
        use std::os::unix::fs::FileExt;
        std::fs::File::open(&disk)
            .unwrap()
            .read_exact_at(&mut sb, 1024)
            .unwrap();
        // A 1 GiB filesystem the guest init grows to the disk's size.
        assert_eq!(cua_microvm_init::ext4_size(&sb), Some((262_144, 4096)));
        assert_eq!(
            cua_microvm_init::ext4_grow_to(262_144, 4096, md.len()),
            Some(4_194_304)
        );
        // Smaller than the image: the image's own size.
        let small = dir.path().join("small.ext4");
        expand_template(SCRATCH_TEMPLATE, &small, 1).unwrap();
        assert_eq!(std::fs::metadata(&small).unwrap().len(), 1 << 30);
        // A damaged template is refused.
        assert!(expand_template(b"nope", &dir.path().join("x"), 1).is_err());
        let mut bad = SCRATCH_TEMPLATE.to_vec();
        bad.truncate(40);
        assert!(expand_template(&bad, &dir.path().join("y"), 1).is_err());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_disk_copy_is_an_apfs_clone_that_diverges() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.img"), dir.path().join("b.img"));
        std::fs::write(&a, vec![7u8; 1 << 20]).unwrap();
        copy_sparse(&a, &b).await.unwrap();
        assert_eq!(std::fs::read(&b).unwrap(), vec![7u8; 1 << 20]);
        std::fs::write(&b, b"changed").unwrap();
        assert_eq!(
            std::fs::read(&a).unwrap(),
            vec![7u8; 1 << 20],
            "the source is untouched"
        );
        assert!(clone_file(&a, &b).is_err(), "never over an existing file");
    }

    #[test]
    fn the_ext4_fixups_restore_owners_modes_and_devices() {
        let s = debugfs_script(&[
            Meta {
                path: "usr/bin/sudo".into(),
                uid: 0,
                gid: 0,
                mode: 0o104755,
                dev: None,
            },
            Meta {
                path: "home/my \"user\"".into(),
                uid: 1000,
                gid: 1000,
                mode: 0o040750,
                dev: None,
            },
            Meta {
                path: "dev/null".into(),
                uid: 0,
                gid: 0,
                mode: 0o020666,
                dev: Some(('c', 1, 3)),
            },
        ]);
        assert!(s.contains("sif \"/usr/bin/sudo\" mode 0104755\n"), "{s}");
        assert!(
            s.contains("sif \"/home/my \\\"user\\\"\" uid 1000\n"),
            "{s}"
        );
        assert!(s.contains("mknod \"/dev/null\" c 1 3\n"), "{s}");
    }

    #[test]
    fn a_dynamic_init_is_told_apart_from_a_static_one() {
        assert_eq!(elf_interpreter(b"#!/bin/sh\n"), None);
        // A minimal ELF64 header with one PT_LOAD program header: static.
        let mut elf = vec![0u8; 64 + 56];
        elf[..6].copy_from_slice(b"\x7fELF\x02\x01");
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[54..56].copy_from_slice(&56u16.to_le_bytes());
        elf[56..58].copy_from_slice(&1u16.to_le_bytes());
        elf[64..68].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(elf_interpreter(&elf), Some(None));
        // The same with PT_INTERP pointing at a loader path.
        let path = b"/lib64/ld-linux-x86-64.so.2\0";
        let off = elf.len() as u64;
        elf.extend_from_slice(path);
        elf[64..68].copy_from_slice(&3u32.to_le_bytes());
        elf[64 + 8..64 + 16].copy_from_slice(&off.to_le_bytes());
        elf[64 + 32..64 + 40].copy_from_slice(&(path.len() as u64).to_le_bytes());
        assert_eq!(
            elf_interpreter(&elf),
            Some(Some("/lib64/ld-linux-x86-64.so.2".into()))
        );
    }

    #[test]
    fn initramfs_is_a_newc_archive_with_the_init() {
        let c = initramfs(b"\x7fELF");
        assert!(c.starts_with(b"070701"));
        let text = String::from_utf8_lossy(&c);
        for name in ["dev/console", "init", "newroot", "TRAILER!!!"] {
            assert!(text.contains(name), "{name}");
        }
        assert_eq!(c.len() % 512, 0);
    }
}
