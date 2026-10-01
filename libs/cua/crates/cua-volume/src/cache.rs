// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The local block cache in front of a remote backend
//! (`$CUA_HOME/volume/cache`).
//!
//! Objects are read in fixed blocks ([`BLOCK`] bytes). A block is keyed by
//! the backend's identity, the object key, the pinned version and the block
//! index, so a new version never serves an old one's bytes and a switched
//! backend never serves another store's. Blocks are files on disk; a read
//! of a few bytes reads only those bytes (`pread`), never the whole block.
//!
//! The cache is size-capped and evicts least recently used blocks first.
//! Concurrent reads of one missing block share a single fetch.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::Digest as _;

use crate::backend::Backend;
use crate::{Error, Result};

/// Bytes per block.
pub const BLOCK: u64 = 1024 * 1024;
/// Default cap: 10 GiB.
pub const DEFAULT_CAPACITY: u64 = 10 * 1024 * 1024 * 1024;
/// Smallest cap accepted.
pub const MIN_CAPACITY: u64 = 256 * 1024 * 1024;

/// What the cache holds and how it has done since it opened.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CacheStats {
    pub dir: String,
    pub size_bytes: u64,
    pub capacity_bytes: u64,
    pub block_bytes: u64,
    pub blocks: u64,
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub prefetched_bytes: u64,
    pub evictions: u64,
}

struct Index {
    /// id -> (size, tick)
    map: HashMap<String, (u64, u64)>,
    /// tick -> id (oldest first)
    lru: BTreeMap<u64, String>,
    total: u64,
    tick: u64,
    capacity: u64,
}

impl Index {
    fn touch(&mut self, id: &str) -> bool {
        let Some((_, t)) = self.map.get(id).copied() else {
            return false;
        };
        self.lru.remove(&t);
        self.tick += 1;
        let tick = self.tick;
        self.lru.insert(tick, id.to_string());
        if let Some(e) = self.map.get_mut(id) {
            e.1 = tick;
        }
        true
    }
}

type Fetch = Arc<tokio::sync::OnceCell<std::result::Result<(), Error>>>;

/// A block cache in one directory.
pub struct BlockCache {
    dir: PathBuf,
    index: Mutex<Index>,
    inflight: Mutex<HashMap<String, Fetch>>,
    hits: AtomicU64,
    misses: AtomicU64,
    prefetched: AtomicU64,
    evictions: AtomicU64,
}

impl std::fmt::Debug for BlockCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockCache")
            .field("dir", &self.dir)
            .finish()
    }
}

/// The id of one block.
pub fn block_id(identity: &str, key: &str, version: &str, block: u64) -> String {
    let mut h = sha2::Sha256::new();
    for part in [identity, key, version] {
        h.update(part.as_bytes());
        h.update([0u8]);
    }
    h.update(block.to_le_bytes());
    hex::encode(&h.finalize()[..20])
}

impl BlockCache {
    /// Opens (or creates) the cache in `dir`, indexing what is already
    /// there (oldest files first), and evicts down to `capacity`.
    pub fn open(dir: impl Into<PathBuf>, capacity: u64) -> Result<BlockCache> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let mut found = vec![];
        for sub in fs::read_dir(&dir)?.flatten() {
            if !sub.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            for f in fs::read_dir(sub.path())?.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                if name.starts_with(".tmp") {
                    let _ = fs::remove_file(f.path());
                    continue;
                }
                if let Ok(m) = f.metadata() {
                    let t = m.modified().ok();
                    found.push((t, name, m.len()));
                }
            }
        }
        found.sort();
        let mut index = Index {
            map: HashMap::new(),
            lru: BTreeMap::new(),
            total: 0,
            tick: 0,
            capacity: capacity.max(MIN_CAPACITY),
        };
        for (_, name, size) in found {
            index.tick += 1;
            index.lru.insert(index.tick, name.clone());
            index.map.insert(name, (size, index.tick));
            index.total += size;
        }
        let cache = BlockCache {
            dir,
            index: Mutex::new(index),
            inflight: Mutex::new(HashMap::new()),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            prefetched: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        };
        cache.evict();
        Ok(cache)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(&id[..2]).join(id)
    }

    /// Whether the block is cached (no counters change).
    pub fn contains(&self, id: &str) -> bool {
        self.index.lock().unwrap().map.contains_key(id)
    }

    /// `len` bytes at `offset` inside cached block `id`, or `None`.
    fn read_cached(&self, id: &str, offset: u64, len: u64) -> Option<Vec<u8>> {
        if !self.index.lock().unwrap().touch(id) {
            return None;
        }
        let f = File::open(self.path(id)).ok()?;
        let size = f.metadata().ok()?.len();
        let start = offset.min(size);
        let want = len.min(size - start) as usize;
        let mut buf = vec![0u8; want];
        let mut done = 0;
        while done < want {
            match read_at(&f, &mut buf[done..], start + done as u64) {
                Ok(0) => break,
                Ok(n) => done += n,
                Err(_) => return None,
            }
        }
        buf.truncate(done);
        Some(buf)
    }

    fn insert(&self, id: &str, bytes: &[u8]) -> Result<()> {
        let path = self.path(id);
        let dir = path.parent().expect("block path has a parent");
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".tmp-{}", crate::new_id()));
        {
            let mut f = File::create(&tmp)?;
            f.write_all(bytes)?;
        }
        fs::rename(&tmp, &path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })?;
        {
            let mut ix = self.index.lock().unwrap();
            if let Some((old, t)) = ix.map.remove(id) {
                ix.lru.remove(&t);
                ix.total -= old;
            }
            ix.tick += 1;
            let tick = ix.tick;
            ix.lru.insert(tick, id.to_string());
            ix.map.insert(id.to_string(), (bytes.len() as u64, tick));
            ix.total += bytes.len() as u64;
        }
        self.evict();
        Ok(())
    }

    fn evict(&self) {
        let mut victims = vec![];
        {
            let mut ix = self.index.lock().unwrap();
            while ix.total > ix.capacity {
                let Some((&t, _)) = ix.lru.iter().next() else {
                    break;
                };
                let id = ix.lru.remove(&t).expect("present");
                if let Some((size, _)) = ix.map.remove(&id) {
                    ix.total -= size;
                }
                victims.push(id);
            }
        }
        for id in victims {
            let _ = fs::remove_file(self.path(&id));
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Sets the cap and evicts down to it at once.
    pub fn set_capacity(&self, capacity: u64) -> Result<()> {
        if capacity < MIN_CAPACITY {
            return Err(Error::Invalid(format!(
                "cache capacity {capacity} is below the minimum {MIN_CAPACITY}"
            )));
        }
        self.index.lock().unwrap().capacity = capacity;
        self.evict();
        Ok(())
    }

    /// Drops every block.
    pub fn clear(&self) {
        let ids: Vec<String> = {
            let mut ix = self.index.lock().unwrap();
            let ids = ix.map.keys().cloned().collect();
            ix.map.clear();
            ix.lru.clear();
            ix.total = 0;
            ids
        };
        for id in ids {
            let _ = fs::remove_file(self.path(&id));
        }
    }

    pub fn stats(&self) -> CacheStats {
        let (size, cap, blocks) = {
            let ix = self.index.lock().unwrap();
            (ix.total, ix.capacity, ix.map.len() as u64)
        };
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        CacheStats {
            dir: self.dir.display().to_string(),
            size_bytes: size,
            capacity_bytes: cap,
            block_bytes: BLOCK,
            blocks,
            hits,
            misses,
            hit_rate: if hits + misses == 0 {
                0.0
            } else {
                hits as f64 / (hits + misses) as f64
            },
            prefetched_bytes: self.prefetched.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }

    /// Fetches block `block` of `key@version` into the cache unless it is
    /// there or on its way. Concurrent callers share one fetch.
    pub async fn ensure(
        &self,
        backend: &dyn Backend,
        key: &str,
        version: &str,
        block: u64,
        prefetch: bool,
    ) -> Result<String> {
        let id = block_id(&backend.identity(), key, version, block);
        if self.contains(&id) {
            return Ok(id);
        }
        let cell = {
            let mut inflight = self.inflight.lock().unwrap();
            inflight.entry(id.clone()).or_default().clone()
        };
        let r = cell
            .get_or_init(|| async {
                if self.contains(&id) {
                    return Ok(());
                }
                let bytes = backend
                    .get_range(key, version, block * BLOCK, BLOCK)
                    .await?;
                if prefetch {
                    self.prefetched
                        .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                }
                self.insert(&id, &bytes)
            })
            .await
            .clone();
        self.inflight.lock().unwrap().remove(&id);
        r.map(|()| id)
    }

    /// Reads `[offset, offset+len)` of `key@version` (an object of `size`
    /// bytes) through the cache, counting a hit or a miss per block.
    pub async fn read(
        &self,
        backend: &dyn Backend,
        key: &str,
        version: &str,
        size: u64,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>> {
        let end = offset.saturating_add(len).min(size);
        if offset >= end {
            return Ok(vec![]);
        }
        let mut out = Vec::with_capacity((end - offset) as usize);
        let identity = backend.identity();
        let mut pos = offset;
        while pos < end {
            let block = pos / BLOCK;
            let within = pos - block * BLOCK;
            let take = (BLOCK - within).min(end - pos);
            let id = block_id(&identity, key, version, block);
            let bytes = match self.read_cached(&id, within, take) {
                Some(b) => {
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    b
                }
                None => {
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    let id = self.ensure(backend, key, version, block, false).await?;
                    match self.read_cached(&id, within, take) {
                        Some(b) => b,
                        // Evicted between the fetch and the read (a tiny
                        // cache under pressure): read the range directly.
                        None => backend.get_range(key, version, pos, take).await?,
                    }
                }
            };
            if bytes.is_empty() {
                break;
            }
            pos += bytes.len() as u64;
            out.extend_from_slice(&bytes);
        }
        Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Condition;
    use crate::fs::FsBackend;

    #[tokio::test]
    async fn reads_hit_after_a_miss_and_the_cap_evicts_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let b = FsBackend::new(dir.path().join("data"));
        let data: Vec<u8> = (0..(3 * BLOCK + 10)).map(|i| (i % 251) as u8).collect();
        let m = b.put("v.bin", data.clone(), Condition::None).await.unwrap();
        let c = BlockCache::open(dir.path().join("cache"), MIN_CAPACITY).unwrap();
        let got = c
            .read(&b, "v.bin", &m.version, m.size, BLOCK - 5, 20)
            .await
            .unwrap();
        assert_eq!(got, &data[(BLOCK - 5) as usize..(BLOCK + 15) as usize]);
        let s = c.stats();
        assert_eq!((s.hits, s.misses, s.blocks), (0, 2, 2));
        let got = c
            .read(&b, "v.bin", &m.version, m.size, BLOCK, 4096)
            .await
            .unwrap();
        assert_eq!(got, &data[BLOCK as usize..(BLOCK + 4096) as usize]);
        assert_eq!(c.stats().hits, 1);
        // The tail is short.
        let got = c
            .read(&b, "v.bin", &m.version, m.size, 3 * BLOCK, BLOCK)
            .await
            .unwrap();
        assert_eq!(got.len(), 10);
        // Reopening indexes what is on disk.
        drop(c);
        let c = BlockCache::open(dir.path().join("cache"), MIN_CAPACITY).unwrap();
        assert_eq!(c.stats().blocks, 3);
        // A new version never serves old blocks.
        let m2 = b
            .put("v.bin", vec![9u8; 100], Condition::None)
            .await
            .unwrap();
        let got = c
            .read(&b, "v.bin", &m2.version, m2.size, 0, 10)
            .await
            .unwrap();
        assert_eq!(got, vec![9u8; 10]);
        // Evict down to a smaller cap (the floor keeps it at MIN_CAPACITY,
        // so force the index cap directly for the test).
        c.index.lock().unwrap().capacity = 2 * BLOCK;
        c.evict();
        let s = c.stats();
        assert!(s.size_bytes <= 2 * BLOCK, "{s:?}");
        assert!(s.evictions >= 1);
        // The newest block (m2's) is kept.
        let id = block_id(&b.identity(), "v.bin", &m2.version, 0);
        assert!(c.contains(&id));
        assert!(c.set_capacity(1).is_err());
        c.clear();
        assert_eq!(c.stats().blocks, 0);
    }

    #[tokio::test]
    async fn concurrent_misses_share_one_fetch() {
        struct Counting(FsBackend, AtomicU64);
        #[async_trait::async_trait]
        impl Backend for Counting {
            fn kind(&self) -> &'static str {
                "counting"
            }
            async fn put(&self, k: &str, b: Vec<u8>, c: Condition) -> Result<crate::ObjectMeta> {
                self.0.put(k, b, c).await
            }
            async fn get(&self, k: &str, v: Option<&str>) -> Result<(Vec<u8>, crate::ObjectMeta)> {
                self.0.get(k, v).await
            }
            async fn head(&self, k: &str) -> Result<Option<crate::ObjectMeta>> {
                self.0.head(k).await
            }
            async fn list(&self, p: &str) -> Result<Vec<crate::ObjectMeta>> {
                self.0.list(p).await
            }
            async fn delete(&self, k: &str, c: Condition) -> Result<()> {
                self.0.delete(k, c).await
            }
            async fn versions(&self, k: &str) -> Result<Vec<crate::VersionInfo>> {
                self.0.versions(k).await
            }
            async fn get_range(&self, k: &str, v: &str, o: u64, l: u64) -> Result<Vec<u8>> {
                self.1.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                self.0.get_range(k, v, o, l).await
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let b = Arc::new(Counting(
            FsBackend::new(dir.path().join("d")),
            AtomicU64::new(0),
        ));
        let m = b.put("x", vec![1u8; 5000], Condition::None).await.unwrap();
        let c = Arc::new(BlockCache::open(dir.path().join("c"), MIN_CAPACITY).unwrap());
        let mut tasks = vec![];
        for i in 0..8u64 {
            let (b, c, v) = (b.clone(), c.clone(), m.version.clone());
            tasks.push(tokio::spawn(async move {
                c.read(b.as_ref(), "x", &v, 5000, i * 10, 10).await.unwrap()
            }));
        }
        for t in tasks {
            assert_eq!(t.await.unwrap(), vec![1u8; 10]);
        }
        assert_eq!(b.1.load(Ordering::SeqCst), 1);
    }
}
