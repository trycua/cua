//! AT-SPI element cache for Linux.
//! Stores element keys (u64 hash) indexed by (pid, xid) → element_index.

use super::AtspiNode;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey { pub pid: u32, pub xid: u64 }

pub struct CachedSnapshot {
    /// element_index → element_key (opaque AT-SPI path hash).
    pub elements: Vec<u64>,
}

pub struct ElementCache {
    inner: Mutex<HashMap<CacheKey, CachedSnapshot>>,
}

impl ElementCache {
    pub fn new() -> Self { Self { inner: Mutex::new(HashMap::new()) } }

    pub fn update(&self, pid: u32, xid: u64, nodes: &[AtspiNode]) {
        let elements: Vec<u64> = nodes.iter()
            .filter(|n| n.element_index.is_some())
            .map(|n| n.element_key)
            .collect();
        self.inner.lock().unwrap().insert(CacheKey { pid, xid }, CachedSnapshot { elements });
    }

    pub fn get_element_key(&self, pid: u32, xid: u64, idx: usize) -> Option<u64> {
        self.inner.lock().unwrap()
            .get(&CacheKey { pid, xid })?
            .elements.get(idx).copied()
    }

    pub fn element_count(&self, pid: u32, xid: u64) -> usize {
        self.inner.lock().unwrap()
            .get(&CacheKey { pid, xid })
            .map(|s| s.elements.len())
            .unwrap_or(0)
    }
}

impl Default for ElementCache { fn default() -> Self { Self::new() } }
