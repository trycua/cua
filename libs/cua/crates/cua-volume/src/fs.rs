// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The local backend: a versioned object store in a directory
//! (`$CUA_HOME/volume/data` by default).
//!
//! ```text
//! <root>/heads/<a>.d/<b>.d/<c>.o          the current version's metadata (JSON)
//! <root>/versions/<a>.d/<b>.d/<c>.o/<v>   every version's bytes (<v>.del: a delete)
//! <root>/.lock                            one writer at a time (all processes)
//! ```
//!
//! Every path component gets a suffix (`.d` for folders, `.o` for objects),
//! so `a` and `a/b` can both exist, as they can in S3. Version ids sort by
//! time. The etag is the content's SHA-256.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::backend::{Backend, Condition, ObjectMeta, VersionInfo};
use crate::{Error, Result, now_ms, sha256_hex};

/// A directory-backed, versioned object store.
#[derive(Clone, Debug)]
pub struct FsBackend {
    root: PathBuf,
}

impl FsBackend {
    /// A store rooted at `root` (created on first write).
    pub fn new(root: impl Into<PathBuf>) -> FsBackend {
        FsBackend { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn encode(key: &str) -> Result<PathBuf> {
        let key = key.trim_end_matches('/');
        if key.is_empty() {
            return Err(Error::Invalid("empty key".into()));
        }
        let parts: Vec<&str> = key.split('/').collect();
        let mut p = PathBuf::new();
        for (i, c) in parts.iter().enumerate() {
            if c.is_empty() || *c == "." || *c == ".." {
                return Err(Error::Invalid(format!("key {key:?}")));
            }
            let suffix = if i + 1 == parts.len() { "o" } else { "d" };
            p.push(format!("{c}.{suffix}"));
        }
        Ok(p)
    }

    fn folder_dir(base: &Path, folder: &str) -> PathBuf {
        let mut p = base.to_path_buf();
        for c in folder.split('/').filter(|c| !c.is_empty()) {
            p.push(format!("{c}.d"));
        }
        p
    }

    fn head_path(&self, key: &str) -> Result<PathBuf> {
        Ok(self.root.join("heads").join(Self::encode(key)?))
    }

    fn versions_dir(&self, key: &str) -> Result<PathBuf> {
        Ok(self.root.join("versions").join(Self::encode(key)?))
    }

    fn lock(&self) -> Result<File> {
        fs::create_dir_all(&self.root)?;
        let path = self.root.join(".lock");
        cua_home::guard_write(&path)?;
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        f.lock()?;
        Ok(f)
    }

    fn read_head(&self, key: &str) -> Result<Option<ObjectMeta>> {
        let p = self.head_path(key)?;
        match fs::read(&p) {
            Ok(b) => Ok(Some(serde_json::from_slice(&b)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
        Self::atomic_write_with(path, bytes, true)
    }

    /// `full`: a full barrier per file (`sync_all`, which is F_FULLFSYNC on
    /// macOS). A batch passes false: each file still reaches the device
    /// (`fsync`), and the batch ends with one full barrier.
    fn atomic_write_with(path: &Path, bytes: &[u8], full: bool) -> Result<()> {
        cua_home::guard_write(path)?;
        let dir = path
            .parent()
            .ok_or_else(|| Error::Invalid("no parent".into()))?;
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".tmp-{}", crate::new_id()));
        {
            let mut f = File::create(&tmp)?;
            f.write_all(bytes)?;
            if full {
                f.sync_all()?;
            } else {
                device_sync(&f)?;
            }
        }
        fs::rename(&tmp, path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })?;
        Ok(())
    }

    fn new_version() -> String {
        format!("{:013}-{:08x}", now_ms(), rand::random::<u32>())
    }

    fn check(cond: &Condition, current: Option<&ObjectMeta>, key: &str) -> Result<()> {
        match (cond, current) {
            (Condition::None, _) => Ok(()),
            (Condition::IfNoneMatch, None) => Ok(()),
            (Condition::IfNoneMatch, Some(_)) => {
                Err(Error::Precondition(format!("{key} already exists")))
            }
            (Condition::IfMatch(want), Some(m)) if &m.etag == want => Ok(()),
            (Condition::IfMatch(_), Some(_)) => Err(Error::Precondition(format!(
                "{key} changed since it was read"
            ))),
            (Condition::IfMatch(_), None) => Err(Error::Precondition(format!("{key} is gone"))),
        }
    }

    fn put_blocking(&self, key: &str, bytes: Vec<u8>, cond: Condition) -> Result<ObjectMeta> {
        let _lock = self.lock()?;
        let current = self.read_head(key)?;
        Self::check(&cond, current.as_ref(), key)?;
        self.put_locked(key, &bytes)
    }

    /// One unconditional write, the store lock already held.
    fn put_locked(&self, key: &str, bytes: &[u8]) -> Result<ObjectMeta> {
        self.put_locked_with(key, bytes, true)
    }

    fn put_locked_with(&self, key: &str, bytes: &[u8], full: bool) -> Result<ObjectMeta> {
        let version = Self::new_version();
        let meta = ObjectMeta {
            key: key.to_string(),
            size: bytes.len() as u64,
            etag: sha256_hex(bytes),
            version: version.clone(),
            modified_ms: now_ms(),
        };
        Self::atomic_write_with(&self.versions_dir(key)?.join(&version), bytes, full)?;
        Self::atomic_write_with(&self.head_path(key)?, &serde_json::to_vec(&meta)?, full)?;
        Ok(meta)
    }

    /// Many unconditional writes under one lock, on a few threads (each file
    /// is still written, synced and renamed into place on its own).
    fn put_many_blocking(&self, items: Vec<(String, Vec<u8>)>) -> Result<Vec<ObjectMeta>> {
        let _lock = self.lock()?;
        const THREADS: usize = 8;
        let chunk = items.len().div_ceil(THREADS).max(1);
        let results: Vec<Result<Vec<ObjectMeta>>> = std::thread::scope(|s| {
            let handles: Vec<_> = items
                .chunks(chunk)
                .map(|part| {
                    s.spawn(move || {
                        part.iter()
                            .map(|(k, b)| self.put_locked_with(k, b, false))
                            .collect::<Result<Vec<_>>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| {
                    h.join()
                        .unwrap_or_else(|_| Err(Error::Backend("drive writer panicked".into())))
                })
                .collect()
        });
        let mut out = vec![];
        for r in results {
            out.extend(r?);
        }
        // One full barrier for the whole batch.
        if !out.is_empty() {
            // Opened for writing: Windows refuses FlushFileBuffers on a
            // read-only handle (Access is denied).
            OpenOptions::new()
                .write(true)
                .open(self.root.join(".lock"))?
                .sync_all()?;
        }
        Ok(out)
    }

    fn delete_blocking(&self, key: &str, cond: Condition) -> Result<()> {
        let _lock = self.lock()?;
        let current = self.read_head(key)?;
        if current.is_none() && cond == Condition::None {
            return Err(Error::NotFound(key.to_string()));
        }
        Self::check(&cond, current.as_ref(), key)?;
        let marker = self
            .versions_dir(key)?
            .join(format!("{}.del", Self::new_version()));
        Self::atomic_write(&marker, b"")?;
        let head = self.head_path(key)?;
        cua_home::guard_write(&head)?;
        fs::remove_file(head)?;
        Ok(())
    }

    fn get_blocking(&self, key: &str, version: Option<&str>) -> Result<(Vec<u8>, ObjectMeta)> {
        let head = self.read_head(key)?;
        match version {
            None => {
                let meta = head.ok_or_else(|| Error::NotFound(key.to_string()))?;
                let bytes = fs::read(self.versions_dir(key)?.join(&meta.version))?;
                Ok((bytes, meta))
            }
            Some(v) => {
                if v.contains('/') || v.contains("..") || v.ends_with(".del") {
                    return Err(Error::Invalid(format!("version {v:?}")));
                }
                let path = self.versions_dir(key)?.join(v);
                let bytes =
                    fs::read(&path).map_err(|_| Error::NotFound(format!("{key} version {v}")))?;
                let modified_ms = version_ms(v);
                Ok((
                    bytes.clone(),
                    ObjectMeta {
                        key: key.to_string(),
                        size: bytes.len() as u64,
                        etag: sha256_hex(&bytes),
                        version: v.to_string(),
                        modified_ms,
                    },
                ))
            }
        }
    }

    fn list_blocking(&self, prefix: &str) -> Result<Vec<ObjectMeta>> {
        let heads = self.root.join("heads");
        let (folder, _) = match prefix.rfind('/') {
            Some(i) => prefix.split_at(i + 1),
            None => ("", prefix),
        };
        let start = Self::folder_dir(&heads, folder);
        let mut out = vec![];
        let mut stack = vec![(start, folder.to_string())];
        // Bounded by the tree on disk; each directory is visited once.
        while let Some((dir, key_prefix)) = stack.pop() {
            let rd = match fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in rd {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some(d) = name.strip_suffix(".d") {
                    stack.push((entry.path(), format!("{key_prefix}{d}/")));
                } else if let Some(o) = name.strip_suffix(".o") {
                    let key = format!("{key_prefix}{o}");
                    if key.starts_with(prefix)
                        && let Ok(b) = fs::read(entry.path())
                        && let Ok(m) = serde_json::from_slice::<ObjectMeta>(&b)
                    {
                        out.push(m);
                    }
                }
            }
        }
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }

    fn versions_blocking(&self, key: &str) -> Result<Vec<VersionInfo>> {
        let dir = self.versions_dir(key)?;
        let head = self.read_head(key)?;
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotFound(key.to_string()));
            }
            Err(e) => return Err(e.into()),
        };
        let mut out = vec![];
        for entry in rd {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".tmp-") {
                continue;
            }
            let (version, deleted) = match name.strip_suffix(".del") {
                Some(v) => (v.to_string(), true),
                None => (name.clone(), false),
            };
            let size = if deleted { 0 } else { entry.metadata()?.len() };
            out.push(VersionInfo {
                latest: !deleted && head.as_ref().is_some_and(|h| h.version == version),
                modified_ms: version_ms(&version),
                version,
                size,
                deleted,
            });
        }
        out.sort_by(|a, b| b.version.cmp(&a.version));
        Ok(out)
    }
}

impl FsBackend {
    fn version_path(&self, key: &str, version: &str) -> Result<PathBuf> {
        if version.is_empty()
            || version.contains('/')
            || version.contains("..")
            || version.ends_with(".del")
            || version.starts_with('.')
        {
            return Err(Error::Invalid(format!("version {version:?}")));
        }
        Ok(self.versions_dir(key)?.join(version))
    }

    fn get_range_blocking(
        &self,
        key: &str,
        version: &str,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>> {
        let path = self.version_path(key, version)?;
        let f = File::open(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound(format!("{key} version {version}")),
            _ => e.into(),
        })?;
        let size = f.metadata()?.len();
        if offset >= size {
            return Ok(vec![]);
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
        Ok(buf)
    }

    /// Copies `src` into the store as a new version of `key`, hashing on the
    /// way; the store lock is held only for the precondition and the rename.
    fn put_file_blocking(&self, key: &str, src: &Path, cond: Condition) -> Result<ObjectMeta> {
        use sha2::Digest as _;
        use std::io::Read as _;
        let dir = self.versions_dir(key)?;
        fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!(".tmp-{}", crate::new_id()));
        cua_home::guard_write(&tmp)?;
        let result = (|| -> Result<(String, u64)> {
            let mut input = File::open(src)?;
            let mut out = File::create(&tmp)?;
            let mut hasher = sha2::Sha256::new();
            let mut buf = vec![0u8; 1 << 20];
            let mut size = 0u64;
            loop {
                let n = input.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n])?;
                size += n as u64;
            }
            out.sync_all()?;
            Ok((hex::encode(hasher.finalize()), size))
        })();
        let (etag, size) = match result {
            Ok(v) => v,
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                return Err(e);
            }
        };
        let _lock = self.lock()?;
        let current = self.read_head(key)?;
        if let Err(e) = Self::check(&cond, current.as_ref(), key) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        let version = Self::new_version();
        fs::rename(&tmp, dir.join(&version)).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })?;
        let meta = ObjectMeta {
            key: key.to_string(),
            size,
            etag,
            version,
            modified_ms: now_ms(),
        };
        Self::atomic_write(&self.head_path(key)?, &serde_json::to_vec(&meta)?)?;
        Ok(meta)
    }

    fn copy_blocking(&self, from: &str, to: &str) -> Result<ObjectMeta> {
        let _lock = self.lock()?;
        let src = self
            .read_head(from)?
            .ok_or_else(|| Error::NotFound(from.to_string()))?;
        let dir = self.versions_dir(to)?;
        fs::create_dir_all(&dir)?;
        let version = Self::new_version();
        let dst = dir.join(&version);
        cua_home::guard_write(&dst)?;
        // A clone on APFS; a copy elsewhere.
        fs::copy(self.versions_dir(from)?.join(&src.version), &dst)?;
        let meta = ObjectMeta {
            key: to.to_string(),
            size: src.size,
            etag: src.etag,
            version,
            modified_ms: now_ms(),
        };
        Self::atomic_write(&self.head_path(to)?, &serde_json::to_vec(&meta)?)?;
        Ok(meta)
    }

    fn has_object(dir: &Path) -> bool {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.ends_with(".o") {
                    return true;
                }
                if name.ends_with(".d") {
                    stack.push(e.path());
                }
            }
        }
        false
    }

    fn list_dir_blocking(&self, folder: &str) -> Result<(Vec<ObjectMeta>, Vec<String>)> {
        let dir = Self::folder_dir(&self.root.join("heads"), folder);
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], vec![])),
            Err(e) => return Err(e.into()),
        };
        let mut files = vec![];
        let mut folders = vec![];
        for entry in rd {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(d) = name.strip_suffix(".d") {
                if Self::has_object(&entry.path()) {
                    folders.push(format!("{folder}{d}/"));
                }
            } else if name.ends_with(".o")
                && let Ok(b) = fs::read(entry.path())
                && let Ok(m) = serde_json::from_slice::<ObjectMeta>(&b)
            {
                files.push(m);
            }
        }
        files.sort_by(|a, b| a.key.cmp(&b.key));
        folders.sort();
        Ok((files, folders))
    }
}

#[cfg(unix)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, offset)
}

#[cfg(windows)]
fn read_at(f: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, offset)
}

/// Pushes a file's data to the device without a full cache barrier.
#[cfg(unix)]
fn device_sync(f: &File) -> Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: a valid open descriptor for the duration of the call.
    if unsafe { libc::fsync(f.as_raw_fd()) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

#[cfg(not(unix))]
fn device_sync(f: &File) -> Result<()> {
    Ok(f.sync_data()?)
}

fn version_ms(v: &str) -> u64 {
    v.split('-')
        .next()
        .and_then(|t| t.parse().ok())
        .unwrap_or(0)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Backend(format!("drive task: {e}")))?
}

#[async_trait::async_trait]
impl Backend for FsBackend {
    fn kind(&self) -> &'static str {
        "fs"
    }

    async fn put(&self, key: &str, bytes: Vec<u8>, cond: Condition) -> Result<ObjectMeta> {
        let (me, key) = (self.clone(), key.to_string());
        blocking(move || me.put_blocking(&key, bytes, cond)).await
    }

    async fn get(&self, key: &str, version: Option<&str>) -> Result<(Vec<u8>, ObjectMeta)> {
        let (me, key, v) = (self.clone(), key.to_string(), version.map(str::to_string));
        blocking(move || me.get_blocking(&key, v.as_deref())).await
    }

    async fn head(&self, key: &str) -> Result<Option<ObjectMeta>> {
        let (me, key) = (self.clone(), key.to_string());
        blocking(move || me.read_head(&key)).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ObjectMeta>> {
        let (me, prefix) = (self.clone(), prefix.to_string());
        blocking(move || me.list_blocking(&prefix)).await
    }

    async fn delete(&self, key: &str, cond: Condition) -> Result<()> {
        let (me, key) = (self.clone(), key.to_string());
        blocking(move || me.delete_blocking(&key, cond)).await
    }

    async fn versions(&self, key: &str) -> Result<Vec<VersionInfo>> {
        let (me, key) = (self.clone(), key.to_string());
        blocking(move || me.versions_blocking(&key)).await
    }

    async fn put_many(&self, items: Vec<(String, Vec<u8>)>) -> Result<Vec<ObjectMeta>> {
        let me = self.clone();
        blocking(move || me.put_many_blocking(items)).await
    }

    async fn get_range(&self, key: &str, version: &str, offset: u64, len: u64) -> Result<Vec<u8>> {
        let (me, key, v) = (self.clone(), key.to_string(), version.to_string());
        blocking(move || me.get_range_blocking(&key, &v, offset, len)).await
    }

    async fn put_file(&self, key: &str, file: &Path, cond: Condition) -> Result<ObjectMeta> {
        let (me, key, file) = (self.clone(), key.to_string(), file.to_path_buf());
        blocking(move || me.put_file_blocking(&key, &file, cond)).await
    }

    async fn list_dir(&self, folder: &str) -> Result<(Vec<ObjectMeta>, Vec<String>)> {
        let (me, folder) = (self.clone(), folder.to_string());
        blocking(move || me.list_dir_blocking(&folder)).await
    }

    async fn copy(&self, from: &str, to: &str) -> Result<ObjectMeta> {
        let (me, from, to) = (self.clone(), from.to_string(), to.to_string());
        blocking(move || me.copy_blocking(&from, &to)).await
    }

    fn identity(&self) -> String {
        format!("fs:{}", self.root.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn versions_preconditions_and_s3_shaped_keys() {
        let dir = tempfile::tempdir().unwrap();
        let b = FsBackend::new(dir.path());
        let m1 = b
            .put("a/b.txt", b"one".to_vec(), Condition::IfNoneMatch)
            .await
            .unwrap();
        assert_eq!(
            b.put("a/b.txt", b"x".to_vec(), Condition::IfNoneMatch)
                .await
                .unwrap_err()
                .tag(),
            "precondition_failed"
        );
        let m2 = b
            .put(
                "a/b.txt",
                b"two".to_vec(),
                Condition::IfMatch(m1.etag.clone()),
            )
            .await
            .unwrap();
        assert_eq!(
            b.put(
                "a/b.txt",
                b"3".to_vec(),
                Condition::IfMatch(m1.etag.clone())
            )
            .await
            .unwrap_err()
            .tag(),
            "precondition_failed"
        );
        // A key and a folder of the same name coexist.
        b.put("a", b"file a".to_vec(), Condition::None)
            .await
            .unwrap();
        assert_eq!(b.get("a", None).await.unwrap().0, b"file a");
        assert_eq!(b.get("a/b.txt", None).await.unwrap().0, b"two");
        assert_eq!(b.get("a/b.txt", Some(&m1.version)).await.unwrap().0, b"one");
        let keys: Vec<String> = b
            .list("")
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.key)
            .collect();
        assert_eq!(keys, ["a", "a/b.txt"]);
        let keys: Vec<String> = b
            .list("a/")
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.key)
            .collect();
        assert_eq!(keys, ["a/b.txt"]);
        b.delete("a/b.txt", Condition::None).await.unwrap();
        assert!(b.head("a/b.txt").await.unwrap().is_none());
        let hist = b.versions("a/b.txt").await.unwrap();
        assert_eq!(hist.len(), 3);
        assert!(hist[0].deleted);
        assert_eq!(hist[1].version, m2.version);
        assert!(!hist.iter().any(|h| h.latest));
        // Restore is a put of an old version.
        let (old, _) = b.get("a/b.txt", Some(&m1.version)).await.unwrap();
        b.put("a/b.txt", old, Condition::IfNoneMatch).await.unwrap();
        assert_eq!(b.get("a/b.txt", None).await.unwrap().0, b"one");
        assert_eq!(
            b.delete("missing", Condition::None)
                .await
                .unwrap_err()
                .tag(),
            "not_found"
        );
        assert!(b.get("a/b.txt", Some("../../x")).await.is_err());
    }

    #[tokio::test]
    async fn concurrent_creates_have_one_winner() {
        let dir = tempfile::tempdir().unwrap();
        let b = FsBackend::new(dir.path());
        let mut tasks = vec![];
        for i in 0..8 {
            let b = b.clone();
            tasks.push(tokio::spawn(async move {
                b.put("lease", format!("{i}").into_bytes(), Condition::IfNoneMatch)
                    .await
            }));
        }
        let mut ok = 0;
        for t in tasks {
            if t.await.unwrap().is_ok() {
                ok += 1;
            }
        }
        assert_eq!(ok, 1);
    }
}
