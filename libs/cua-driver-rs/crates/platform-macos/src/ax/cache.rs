//! Per-(pid, window_id) element cache.
//!
//! After `get_window_state`, each actionable element's AXUIElementRef pointer
//! is cached by element_index. Subsequent `click`, `type_text`, etc. look up
//! the element_index to get the raw pointer and perform AX actions on it.
//!
//! Cache is scoped per (pid, window_id) — a new `get_window_state` call
//! for the same (pid, window_id) replaces the entire entry.
//!
//! Memory contract:
//!   tree::walk_element retains each actionable element before storing its ptr.
//!   CachedSnapshot::drop releases those retains so we have no AX leaks.

use super::bindings::AXUIElementRef;
use super::tree::AXNode;
use core_foundation::base::{CFRelease, CFTypeRef};
use std::collections::HashMap;
use std::sync::Mutex;

/// Key for the element cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub pid: i32,
    pub window_id: u32,
}

/// Cached snapshot for one (pid, window_id) pair.
pub struct CachedSnapshot {
    /// element_index → raw AXUIElementRef pointer (retained, as usize for Send).
    pub elements: Vec<usize>,
}

impl Drop for CachedSnapshot {
    fn drop(&mut self) {
        // Release the extra CFRetain that walk_element added for each cached ptr.
        for ptr in &self.elements {
            if *ptr != 0 {
                unsafe { CFRelease(*ptr as AXUIElementRef as CFTypeRef) };
            }
        }
    }
}

/// Global element cache.
pub struct ElementCache {
    inner: Mutex<HashMap<CacheKey, CachedSnapshot>>,
}

impl ElementCache {
    pub fn new() -> Self {
        Self { inner: Mutex::new(HashMap::new()) }
    }

    /// Replace the snapshot for (pid, window_id) with the nodes from a fresh walk.
    pub fn update(&self, pid: i32, window_id: u32, nodes: &[AXNode]) {
        let elements: Vec<usize> = nodes
            .iter()
            .filter(|n| n.element_index.is_some())
            .map(|n| n.element_ptr)
            .collect();
        let mut inner = self.inner.lock().unwrap();
        inner.insert(CacheKey { pid, window_id }, CachedSnapshot { elements });
    }

    /// Look up the raw AXUIElementRef pointer for `element_index` in (pid, window_id).
    pub fn get_element_ptr(&self, pid: i32, window_id: u32, element_index: usize) -> Option<usize> {
        let inner = self.inner.lock().unwrap();
        inner.get(&CacheKey { pid, window_id })?.elements.get(element_index).copied()
    }

    /// Number of indexed elements for (pid, window_id), or 0 if not cached.
    pub fn element_count(&self, pid: i32, window_id: u32) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.get(&CacheKey { pid, window_id }).map(|s| s.elements.len()).unwrap_or(0)
    }
}

impl Default for ElementCache {
    fn default() -> Self {
        Self::new()
    }
}
