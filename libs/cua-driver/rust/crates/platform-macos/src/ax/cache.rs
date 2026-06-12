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
//!
//! The locked-HashMap plumbing lives in `cua_driver_core::element_cache` — see
//! `docs/dedup-audit.md` item #3. This module owns the macOS-specific
//! `CacheKey`, `CachedSnapshot`, and the `Drop` impl that fires `CFRelease`
//! when an entry is replaced or removed.

use super::bindings::AXUIElementRef;
use super::tree::AXNode;
use core_foundation::base::{CFRelease, CFRetain, CFTypeRef};
use cua_driver_core::element_cache::ElementCacheCore;

/// An AXUIElementRef borrowed out of the cache with an extra `CFRetain`, so it
/// stays alive for the duration of an AX action even if a concurrent
/// `get_window_state` (→ [`ElementCache::update`]) replaces and drops the
/// snapshot it came from. Without this, the snapshot's `Drop` could `CFRelease`
/// the element to zero while an in-flight click was still dereferencing the raw
/// pointer — a use-after-free that trips `AXUIElementCopyActionNames` →
/// `CFGetTypeID` (`EXC_BREAKPOINT`) and crashes the daemon. The retain is taken
/// under the cache lock (see [`ElementCache::get_element_retained`]); the
/// matching `CFRelease` fires on drop.
pub struct RetainedElement(usize);

impl RetainedElement {
    /// The raw pointer, valid for as long as this guard is held.
    pub fn as_ptr(&self) -> usize {
        self.0
    }
}

// The raw AXUIElementRef is already shuttled across threads as a `usize` into
// `spawn_blocking`; wrapping it in a retain guard doesn't change that, and CF
// reference counting is thread-safe, so the guard is safe to Send.
unsafe impl Send for RetainedElement {}

impl Drop for RetainedElement {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { CFRelease(self.0 as AXUIElementRef as CFTypeRef) };
        }
    }
}

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
    core: ElementCacheCore<CacheKey, CachedSnapshot>,
}

impl ElementCache {
    pub fn new() -> Self {
        Self { core: ElementCacheCore::new() }
    }

    /// Replace the snapshot for (pid, window_id) with the nodes from a fresh walk.
    pub fn update(&self, pid: i32, window_id: u32, nodes: &[AXNode]) {
        let elements: Vec<usize> = nodes
            .iter()
            .filter(|n| n.element_index.is_some())
            .map(|n| n.element_ptr)
            .collect();
        self.core.insert(CacheKey { pid, window_id }, CachedSnapshot { elements });
    }

    /// Look up + `CFRetain` the element for `element_index` in (pid, window_id),
    /// returning a guard that releases on drop. The retain happens **under the
    /// cache lock**, so a concurrent [`update`](Self::update) (which replaces
    /// the snapshot and drops its retains) cannot free the element between the
    /// lookup and the retain. Hold the returned guard for the entire AX action —
    /// this is what makes element actions safe when two sessions drive the same
    /// `(pid, window_id)`. Returns `None` if the index isn't cached.
    pub fn get_element_retained(
        &self,
        pid: i32,
        window_id: u32,
        element_index: usize,
    ) -> Option<RetainedElement> {
        self.core
            .with_snapshot(&CacheKey { pid, window_id }, |s| {
                let ptr = s.elements.get(element_index).copied()?;
                if ptr != 0 {
                    // Safety: still inside `with_snapshot`'s lock, so the
                    // snapshot (and thus this CFTypeRef) is alive right now.
                    unsafe { CFRetain(ptr as AXUIElementRef as CFTypeRef) };
                }
                Some(RetainedElement(ptr))
            })
            .flatten()
    }

    /// Number of indexed elements for (pid, window_id), or 0 if not cached.
    pub fn element_count(&self, pid: i32, window_id: u32) -> usize {
        self.core
            .with_snapshot(&CacheKey { pid, window_id }, |s| s.elements.len())
            .unwrap_or(0)
    }
}

impl Default for ElementCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::base::{CFGetRetainCount, CFRetain, TCFType};
    use core_foundation::string::CFString;

    // An AXNode carrying a raw CFTypeRef pointer as if it were an element.
    // A long, dynamic string is heap-allocated (not a tagged-pointer CFString),
    // so CFGetRetainCount is reliable.
    fn node_with_ptr(ptr: usize) -> AXNode {
        AXNode {
            element_index: Some(0),
            role: String::new(),
            title: None,
            value: None,
            description: None,
            identifier: None,
            help: None,
            actions: Vec::new(),
            element_ptr: ptr,
        }
    }

    /// The crash this guards against: while a click holds an element pointer,
    /// a concurrent `get_window_state` replaces the snapshot and its `Drop`
    /// `CFRelease`s the element to zero — freeing it under the in-flight click
    /// (use-after-free → `EXC_BREAKPOINT` in `AXUIElementCopyActionNames`).
    /// `get_element_retained` takes an extra retain under the lock so the
    /// element stays alive across the replace. This asserts that accounting.
    #[test]
    fn retained_element_survives_concurrent_snapshot_replace() {
        let s = CFString::new("cua-driver-uaf-test-element-placeholder");
        let ptr = s.as_concrete_TypeRef() as usize;
        let base = unsafe { CFGetRetainCount(ptr as CFTypeRef) };

        // walk_element's contract: the producer retains before handing the ptr
        // to the cache, and CachedSnapshot::drop releases that retain.
        unsafe { CFRetain(ptr as CFTypeRef) };
        let cache = ElementCache::new();
        cache.update(1, 2, &[node_with_ptr(ptr)]);
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base + 1, "cache owns one retain");

        // Borrow the element out for an action.
        let guard = cache.get_element_retained(1, 2, 0).expect("element is cached");
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base + 2, "guard adds a retain");

        // Concurrent get_window_state replaces the snapshot → old one dropped →
        // CFRelease of the cache's retain. The guard's retain must remain.
        cache.update(1, 2, &[]);
        assert_eq!(
            unsafe { CFGetRetainCount(ptr as CFTypeRef) },
            base + 1,
            "after the replace, only the guard's retain remains — the element is still ALIVE \
             (pre-fix this would drop to `base` and a real AX element with no other owner would be freed)"
        );

        drop(guard);
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base, "guard drop releases its retain");
    }

    /// A missing index returns None without retaining anything.
    #[test]
    fn missing_index_returns_none() {
        let cache = ElementCache::new();
        assert!(cache.get_element_retained(1, 2, 0).is_none());
        cache.update(1, 2, &[]);
        assert!(cache.get_element_retained(1, 2, 5).is_none());
    }
}
