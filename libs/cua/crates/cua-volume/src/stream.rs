// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Streaming reads: any byte range of any object, without downloading it.
//!
//! A [`Streamer`] reads ranges of a pinned version. On a local backend it
//! reads the version file directly (`pread`). On a remote backend it reads
//! through the [`BlockCache`] and watches each stream's access pattern:
//! sequential reads (a player, an editor scrubbing forward, `ffmpeg`)
//! double a read-ahead window up to [`MAX_WINDOW`], fetched in the
//! background with bounded concurrency, so the next reads hit the cache.
//! A random read resets the window, so seeks do not waste bandwidth.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::Result;
use crate::backend::Backend;
use crate::cache::{BLOCK, BlockCache};

/// First read-ahead window after a sequential read is seen.
pub const MIN_WINDOW: u64 = 4 * BLOCK;
/// Largest read-ahead window.
pub const MAX_WINDOW: u64 = 64 * BLOCK;
/// Blocks fetched in the background at once (per streamer).
pub const PREFETCH_CONCURRENCY: usize = 16;
/// How far from the furthest read a request may land and still count as
/// sequential (kernel read-ahead arrives in parallel and out of order).
const SLACK: u64 = 32 * BLOCK;
/// Streams whose pattern is remembered.
const MAX_STREAMS: usize = 256;

#[derive(Clone, Copy, Debug, Default)]
struct Pattern {
    next: u64,
    window: u64,
    /// Blocks below this index were already requested.
    requested_to: u64,
    tick: u64,
}

/// Reads ranges of pinned versions, with a cache and read-ahead for remote
/// backends.
pub struct Streamer {
    backend: Arc<dyn Backend>,
    cache: Option<Arc<BlockCache>>,
    patterns: Mutex<HashMap<(String, String), Pattern>>,
    prefetch: Arc<tokio::sync::Semaphore>,
    tick: AtomicU64,
    read_bytes: AtomicU64,
}

impl std::fmt::Debug for Streamer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Streamer")
            .field("backend", &self.backend.kind())
            .field("cache", &self.cache.is_some())
            .finish()
    }
}

impl Streamer {
    /// A streamer over `backend`; `cache` is used only when the backend is
    /// remote.
    pub fn new(backend: Arc<dyn Backend>, cache: Option<Arc<BlockCache>>) -> Streamer {
        let cache = if backend.remote() { cache } else { None };
        Streamer {
            backend,
            cache,
            patterns: Mutex::new(HashMap::new()),
            prefetch: Arc::new(tokio::sync::Semaphore::new(PREFETCH_CONCURRENCY)),
            tick: AtomicU64::new(0),
            read_bytes: AtomicU64::new(0),
        }
    }

    pub fn backend(&self) -> &Arc<dyn Backend> {
        &self.backend
    }

    pub fn cache(&self) -> Option<&Arc<BlockCache>> {
        self.cache.as_ref()
    }

    /// Bytes returned by [`Streamer::read`] so far.
    pub fn read_bytes(&self) -> u64 {
        self.read_bytes.load(Ordering::Relaxed)
    }

    /// `[offset, offset+len)` of `key@version` (an object of `size` bytes).
    pub async fn read(
        &self,
        key: &str,
        version: &str,
        size: u64,
        offset: u64,
        len: u64,
    ) -> Result<Vec<u8>> {
        let len = len.min(size.saturating_sub(offset));
        if len == 0 {
            return Ok(vec![]);
        }
        let out = match &self.cache {
            None => self.backend.get_range(key, version, offset, len).await?,
            Some(cache) => {
                self.plan_readahead(cache, key, version, size, offset, len);
                cache
                    .read(self.backend.as_ref(), key, version, size, offset, len)
                    .await?
            }
        };
        self.read_bytes
            .fetch_add(out.len() as u64, Ordering::Relaxed);
        Ok(out)
    }

    /// Updates the stream's pattern and starts background fetches for the
    /// blocks ahead of a sequential reader.
    fn plan_readahead(
        &self,
        cache: &Arc<BlockCache>,
        key: &str,
        version: &str,
        size: u64,
        offset: u64,
        len: u64,
    ) {
        let end = offset + len;
        let tick = self.tick.fetch_add(1, Ordering::Relaxed);
        let (from, to) = {
            let mut pats = self.patterns.lock().unwrap();
            if pats.len() >= MAX_STREAMS {
                // Forget the least recently used half.
                let mut ticks: Vec<u64> = pats.values().map(|p| p.tick).collect();
                ticks.sort_unstable();
                let cut = ticks[ticks.len() / 2];
                pats.retain(|_, p| p.tick > cut);
            }
            let p = pats
                .entry((key.to_string(), version.to_string()))
                .or_default();
            // Sequential: near the furthest point read so far. Kernel
            // clients (NFS, FUSE) send their own read-ahead as parallel,
            // out-of-order requests, so "near" is a window, not an exact
            // match. The first read of a stream at offset 0 counts too:
            // players and probes start there and keep going.
            let near = offset + SLACK >= p.next && offset <= p.next + SLACK;
            let sequential = (p.next > 0 && near) || (p.next == 0 && offset == 0);
            if sequential {
                p.window = (p.window * 2).clamp(MIN_WINDOW, MAX_WINDOW);
                p.next = p.next.max(end);
            } else {
                p.window = 0;
                p.requested_to = 0;
                p.next = end;
            }
            p.tick = tick;
            if p.window == 0 {
                return;
            }
            let end = p.next;
            let first = end.div_ceil(BLOCK).max(p.requested_to);
            let last = (end + p.window).min(size).div_ceil(BLOCK);
            if first >= last {
                return;
            }
            p.requested_to = last;
            (first, last)
        };
        for block in from..to {
            let (cache, backend, key, version, sem) = (
                cache.clone(),
                self.backend.clone(),
                key.to_string(),
                version.to_string(),
                self.prefetch.clone(),
            );
            let id = crate::cache::block_id(&backend.identity(), &key, &version, block);
            if cache.contains(&id) {
                continue;
            }
            tokio::spawn(async move {
                let Ok(_permit) = sem.acquire_owned().await else {
                    return;
                };
                let _ = cache
                    .ensure(backend.as_ref(), &key, &version, block, true)
                    .await;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Condition;
    use crate::fs::FsBackend;

    /// A local backend that says it is remote, so the cache path runs.
    struct Remote(FsBackend);
    #[async_trait::async_trait]
    impl Backend for Remote {
        fn kind(&self) -> &'static str {
            "remote-test"
        }
        fn remote(&self) -> bool {
            true
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
            self.0.get_range(k, v, o, l).await
        }
    }

    #[tokio::test]
    async fn sequential_reads_prefetch_and_hit_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let b = Arc::new(Remote(FsBackend::new(dir.path().join("d"))));
        let data: Vec<u8> = (0..(40 * BLOCK)).map(|i| (i % 253) as u8).collect();
        let m = b
            .put("movie.mp4", data.clone(), Condition::None)
            .await
            .unwrap();
        let cache =
            Arc::new(BlockCache::open(dir.path().join("c"), crate::cache::MIN_CAPACITY).unwrap());
        let s = Streamer::new(b.clone(), Some(cache.clone()));
        let chunk = 256 * 1024;
        let mut off = 0;
        while off < 20 * BLOCK {
            let got = s
                .read("movie.mp4", &m.version, m.size, off, chunk)
                .await
                .unwrap();
            assert_eq!(got, &data[off as usize..(off + chunk) as usize]);
            off += chunk;
            // Let background fetches land.
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let st = cache.stats();
        assert!(st.prefetched_bytes >= 8 * BLOCK, "{st:?}");
        assert!(st.hit_rate > 0.9, "{st:?}");
        // A seek to the middle then reads on: correct bytes.
        let mid = 30 * BLOCK + 17;
        let got = s
            .read("movie.mp4", &m.version, m.size, mid, 4096)
            .await
            .unwrap();
        assert_eq!(got, &data[mid as usize..mid as usize + 4096]);
        // Past the end is empty; a read over the end is short.
        assert!(
            s.read("movie.mp4", &m.version, m.size, m.size, 10)
                .await
                .unwrap()
                .is_empty()
        );
        let got = s
            .read("movie.mp4", &m.version, m.size, m.size - 5, 10)
            .await
            .unwrap();
        assert_eq!(got.len(), 5);
    }

    #[tokio::test]
    async fn a_local_backend_reads_directly() {
        let dir = tempfile::tempdir().unwrap();
        let b = Arc::new(FsBackend::new(dir.path().join("d")));
        let m = b
            .put("a", b"hello world".to_vec(), Condition::None)
            .await
            .unwrap();
        let cache =
            Arc::new(BlockCache::open(dir.path().join("c"), crate::cache::MIN_CAPACITY).unwrap());
        let s = Streamer::new(b, Some(cache.clone()));
        assert!(s.cache().is_none());
        assert_eq!(
            s.read("a", &m.version, m.size, 6, 5).await.unwrap(),
            b"world"
        );
        assert_eq!(cache.stats().blocks, 0);
    }
}
