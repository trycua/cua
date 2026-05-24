//! AT-SPI element cache for Linux.
//! Stores element keys (u64 hash) indexed by (pid, xid) → element_index.
//!
//! The locked-HashMap plumbing lives in `mcp_server::element_cache` — see
//! `docs/dedup-audit.md` item #3. This module owns the Linux-specific
//! `CacheKey` and `CachedSnapshot` (no Drop needed — `Vec<u64>` frees
//! itself).

use super::AtspiNode;
use mcp_server::element_cache::ElementCacheCore;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey { pub pid: u32, pub xid: u64 }

pub struct CachedSnapshot {
    /// element_index → element_key (opaque AT-SPI path hash).
    pub elements: Vec<u64>,
}

pub struct ElementCache {
    core: ElementCacheCore<CacheKey, CachedSnapshot>,
}

impl ElementCache {
    pub fn new() -> Self { Self { core: ElementCacheCore::new() } }

    pub fn update(&self, pid: u32, xid: u64, nodes: &[AtspiNode]) {
        let elements: Vec<u64> = nodes.iter()
            .filter(|n| n.element_index.is_some())
            .map(|n| n.element_key)
            .collect();
        self.core.insert(CacheKey { pid, xid }, CachedSnapshot { elements });
    }

    pub fn get_element_key(&self, pid: u32, xid: u64, idx: usize) -> Option<u64> {
        self.core
            .with_snapshot(&CacheKey { pid, xid }, |s| s.elements.get(idx).copied())
            .flatten()
    }

    pub fn element_count(&self, pid: u32, xid: u64) -> usize {
        self.core
            .with_snapshot(&CacheKey { pid, xid }, |s| s.elements.len())
            .unwrap_or(0)
    }
}

impl Default for ElementCache { fn default() -> Self { Self::new() } }
