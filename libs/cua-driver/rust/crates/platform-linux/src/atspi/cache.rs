//! AT-SPI element cache for Linux.
//! Stores, per (pid, xid) snapshot, what a later per-index action needs to
//! act WITHOUT re-walking the application: the element's D-Bus identity
//! ([`native::ObjectRef`]), its role, and the screen frame the snapshot
//! reported. Re-walking large trees (LibreOffice, GIMP, Nautilus) was the
//! multi-second stall behind every element-index click.
//!
//! The locked-HashMap plumbing lives in `cua_driver_core::element_cache` — see
//! `docs/dedup-audit.md` item #3. This module owns the Linux-specific
//! `CacheKey` and `CachedSnapshot` (no Drop needed).
//!
//! The store is process-global: the free functions in `atspi` (perform_action,
//! get_element_bounds, …) consult it by (pid, xid) without needing the tool
//! state threaded through every input path.

use super::native::{self, ObjectRef};
use super::AtspiNode;
use cua_driver_core::element_cache::ElementCacheCore;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub pid: u32,
    pub xid: u64,
}

/// One indexed element as the last `get_window_state` snapshot saw it.
#[derive(Debug, Clone)]
pub struct CachedElement {
    /// Opaque AT-SPI path hash (legacy `element_key`).
    pub key: u64,
    pub object_ref: Option<ObjectRef>,
    pub role: String,
    /// The node lives inside an embedded web document (bounds need the
    /// document origin, which only a walk resolves).
    pub in_web_content: bool,
    /// Screen-space frame `(x, y, w, h)` when the snapshot resolved one.
    pub bounds: Option<(i32, i32, u32, u32)>,
}

pub struct CachedSnapshot {
    /// element_index → cached element.
    pub elements: Vec<CachedElement>,
}

fn store() -> &'static ElementCacheCore<CacheKey, CachedSnapshot> {
    static STORE: OnceLock<ElementCacheCore<CacheKey, CachedSnapshot>> = OnceLock::new();
    STORE.get_or_init(ElementCacheCore::new)
}

/// Known xids for `pid`, most recently updated first is not tracked; callers
/// that lack an xid get the first snapshot found for the pid.
fn keys_for_pid(pid: u32) -> Vec<CacheKey> {
    store().keys().into_iter().filter(|k| k.pid == pid).collect()
}

/// Handle to the process-global snapshot store (kept as a struct so
/// `ToolState` ownership and the existing call sites stay unchanged).
pub struct ElementCache;

impl ElementCache {
    pub fn new() -> Self {
        Self
    }

    /// Replace the snapshot for (pid, xid). `bounds` are the screen frames the
    /// same walk produced, keyed by element_index.
    pub fn update(&self, pid: u32, xid: u64, nodes: &[AtspiNode]) {
        update_snapshot(pid, xid, nodes, &[]);
    }

    pub fn update_with_bounds(
        &self,
        pid: u32,
        xid: u64,
        nodes: &[AtspiNode],
        bounds: &[(usize, i32, i32, u32, u32)],
    ) {
        update_snapshot(pid, xid, nodes, bounds);
    }

    pub fn get_element_key(&self, pid: u32, xid: u64, idx: usize) -> Option<u64> {
        store()
            .with_snapshot(&CacheKey { pid, xid }, |s| s.elements.get(idx).map(|e| e.key))
            .flatten()
    }

    pub fn element_count(&self, pid: u32, xid: u64) -> usize {
        store()
            .with_snapshot(&CacheKey { pid, xid }, |s| s.elements.len())
            .unwrap_or(0)
    }
}

impl Default for ElementCache {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn update_snapshot(
    pid: u32,
    xid: u64,
    nodes: &[AtspiNode],
    bounds: &[(usize, i32, i32, u32, u32)],
) {
    let mut elements: Vec<CachedElement> = nodes
        .iter()
        .filter(|n| n.element_index.is_some())
        .map(|n| CachedElement {
            key: n.element_key,
            object_ref: n.object_ref.clone(),
            role: n.role.clone(),
            in_web_content: n.in_web_content,
            bounds: None,
        })
        .collect();
    for &(idx, x, y, w, h) in bounds {
        if let Some(element) = elements.get_mut(idx) {
            element.bounds = Some((x, y, w, h));
        }
    }
    store().insert(CacheKey { pid, xid }, CachedSnapshot { elements });
}

/// The cached element at `idx` for (pid, xid); with `xid == None`, the first
/// snapshot recorded for `pid`.
pub(crate) fn cached_element(pid: u32, xid: Option<u64>, idx: usize) -> Option<CachedElement> {
    let keys = match xid {
        Some(xid) => vec![CacheKey { pid, xid }],
        None => keys_for_pid(pid),
    };
    keys.into_iter().find_map(|key| {
        store()
            .with_snapshot(&key, |s| s.elements.get(idx).cloned())
            .flatten()
    })
}

/// Hit-test a *screen* point against the cached frames of (pid, xid): the
/// smallest covering real actuator wins, then a passive label. Returns the
/// element index and its cached identity.
pub(crate) fn hit_test(pid: u32, xid: u64, sx: i32, sy: i32) -> Option<(usize, CachedElement)> {
    store()
        .with_snapshot(&CacheKey { pid, xid }, |s| {
            let frames: Vec<(usize, i32, i32, u32, u32, bool)> = s
                .elements
                .iter()
                .enumerate()
                .filter_map(|(idx, e)| {
                    let (x, y, w, h) = e.bounds?;
                    (w > 0 && h > 0).then_some((idx, x, y, w, h, native::is_passive_role(&e.role)))
                })
                .collect();
            let idx = native::select_click_target(&frames, sx, sy)?;
            Some((idx, s.elements[idx].clone()))
        })
        .flatten()
}

/// Drop every snapshot for `pid` (e.g. after the process exited).
#[allow(dead_code)]
pub(crate) fn forget_pid(pid: u32) {
    for key in keys_for_pid(pid) {
        store().remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(idx: usize, role: &str) -> AtspiNode {
        AtspiNode {
            element_index: Some(idx),
            role: role.into(),
            name: None,
            value: None,
            checked: None,
            enabled: Some(true),
            selected: None,
            description: None,
            actions: vec!["click".into()],
            element_key: idx as u64,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
            object_ref: Some(ObjectRef {
                bus: ":1.9".into(),
                path: format!("/org/a11y/atspi/accessible/{idx}"),
            }),
        }
    }

    #[test]
    fn snapshot_hit_test_prefers_smallest_real_actuator_and_keeps_refs() {
        let pid = 424_242;
        let nodes = vec![node(0, "panel"), node(1, "push button"), node(2, "label")];
        update_snapshot(
            pid,
            77,
            &nodes,
            &[(0, 0, 0, 500, 500), (1, 100, 100, 50, 20), (2, 105, 102, 40, 16)],
        );
        let (idx, element) = hit_test(pid, 77, 110, 110).expect("hit");
        assert_eq!(idx, 1);
        assert_eq!(
            element.object_ref.as_ref().map(|r| r.path.as_str()),
            Some("/org/a11y/atspi/accessible/1")
        );
        assert!(hit_test(pid, 77, 900, 900).is_none());
        // Index lookup without an xid falls back to any snapshot for the pid.
        assert_eq!(cached_element(pid, None, 2).unwrap().role, "label");
        assert!(cached_element(pid, Some(78), 2).is_none());
        forget_pid(pid);
        assert!(cached_element(pid, None, 0).is_none());
    }
}
