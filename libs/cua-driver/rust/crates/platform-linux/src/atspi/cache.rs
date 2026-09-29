//! AT-SPI element cache for Linux.
//!
//! Two views of the same `get_window_state` snapshot live here:
//!
//! 1. [`CachedSnapshot`], the [`SnapshotPayload`] published into the runtime
//!    element cache (`cua_driver_core::element_cache`). An `element_token` /
//!    `element_index` resolves through it to a [`CachedElement`]: the
//!    element's proven AT-SPI identity ([`AtspiIdentity`]) plus what a later
//!    per-index action needs to act WITHOUT re-walking the application (its
//!    D-Bus address ([`native::ObjectRef`]), role, and the screen frame the
//!    snapshot reported). Re-walking large trees (LibreOffice, GIMP, Nautilus)
//!    was the multi-second stall behind every element-index click.
//!
//! 2. A process-global (pid, xid) side index over the same elements, so the
//!    free functions in `atspi` (perform_action_in, get_element_bounds,
//!    perform_action_at_point_in, ...) can consult the last snapshot by window
//!    without the tool state threaded through every input path.
//!
//! Membership follows the runtime cache rule: only nodes with an
//! `element_index` AND a proven identity are addressable. An unproven
//! identity (a well-known bus name whose owner could not be pinned) is
//! discovery-only.

use super::native::{self, ObjectRef};
use super::{AtspiIdentity, AtspiNode};
use cua_driver_core::element_cache::{ElementCacheCore, SnapshotPayload};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

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
    /// Proven identity (unique bus owner + owning frame): what an indexed
    /// click acts on; the live ordinal is never used to retarget.
    pub identity: AtspiIdentity,
    /// D-Bus address for the cached fast paths (same object as `identity`).
    pub object_ref: Option<ObjectRef>,
    pub role: String,
    /// The node lives inside an embedded web document (bounds need the
    /// document origin, which only a walk resolves).
    pub in_web_content: bool,
    /// Screen-space frame `(x, y, w, h)` when the snapshot resolved one.
    pub bounds: Option<(i32, i32, u32, u32)>,
}

pub struct CachedSnapshot {
    elements: Arc<HashMap<usize, CachedElement>>,
}

impl CachedSnapshot {
    pub fn from_nodes(nodes: &[AtspiNode]) -> Self {
        Self::from_nodes_with_bounds(nodes, &[])
    }

    /// `bounds` are the screen frames the same walk produced, keyed by
    /// element_index.
    pub fn from_nodes_with_bounds(
        nodes: &[AtspiNode],
        bounds: &[(usize, i32, i32, u32, u32)],
    ) -> Self {
        let mut elements: HashMap<usize, CachedElement> = nodes
            .iter()
            .filter_map(|node| {
                let index = node.element_index?;
                let identity = node.identity.clone()?;
                Some((
                    index,
                    CachedElement {
                        key: node.element_key,
                        identity,
                        object_ref: node.object_ref.clone(),
                        role: node.role.clone(),
                        in_web_content: node.in_web_content,
                        bounds: None,
                    },
                ))
            })
            .collect();
        for &(idx, x, y, w, h) in bounds {
            if let Some(element) = elements.get_mut(&idx) {
                element.bounds = Some((x, y, w, h));
            }
        }
        Self {
            elements: Arc::new(elements),
        }
    }
}

impl SnapshotPayload for CachedSnapshot {
    type Element = CachedElement;
    fn len(&self) -> usize {
        self.elements.len()
    }
    fn retain(&self, index: usize) -> Option<CachedElement> {
        self.elements.get(&index).cloned()
    }
}

pub type ElementCache = ElementCacheCore<CachedSnapshot>;

// -- (pid, xid) side index ----------------------------------------------------

type SideIndex = HashMap<CacheKey, Arc<HashMap<usize, CachedElement>>>;

fn store() -> &'static Mutex<SideIndex> {
    static STORE: OnceLock<Mutex<SideIndex>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Build the payload for (pid, xid) and record its elements in the side index.
/// Returns the payload for the caller to publish into the runtime cache.
pub(crate) fn update_snapshot(
    pid: u32,
    xid: u64,
    nodes: &[AtspiNode],
    bounds: &[(usize, i32, i32, u32, u32)],
) -> CachedSnapshot {
    let snapshot = CachedSnapshot::from_nodes_with_bounds(nodes, bounds);
    store()
        .lock()
        .unwrap()
        .insert(CacheKey { pid, xid }, snapshot.elements.clone());
    snapshot
}

/// Forget the side-index entry for (pid, xid) (the runtime cache is retired
/// separately by the owning `ElementCache`).
pub(crate) fn forget_window(pid: u32, xid: u64) {
    store().lock().unwrap().remove(&CacheKey { pid, xid });
}

fn snapshot_for(pid: u32, xid: Option<u64>) -> Option<Arc<HashMap<usize, CachedElement>>> {
    let store = store().lock().unwrap();
    match xid {
        Some(xid) => store.get(&CacheKey { pid, xid }).cloned(),
        // Callers that lack an xid get the first snapshot found for the pid.
        None => store
            .iter()
            .find(|(key, _)| key.pid == pid)
            .map(|(_, elements)| elements.clone()),
    }
}

/// The cached element at `idx` for (pid, xid); with `xid == None`, the first
/// snapshot recorded for `pid`.
pub(crate) fn cached_element(pid: u32, xid: Option<u64>, idx: usize) -> Option<CachedElement> {
    snapshot_for(pid, xid)?.get(&idx).cloned()
}

/// Hit-test a *screen* point against the cached frames of (pid, xid): the
/// smallest covering real actuator wins, then a passive label. Returns the
/// element index and its cached identity.
pub(crate) fn hit_test(pid: u32, xid: u64, sx: i32, sy: i32) -> Option<(usize, CachedElement)> {
    let elements = snapshot_for(pid, Some(xid))?;
    let mut indexed: Vec<(&usize, &CachedElement)> = elements.iter().collect();
    indexed.sort_by_key(|(idx, _)| **idx);
    let frames: Vec<(usize, i32, i32, u32, u32, bool)> = indexed
        .iter()
        .enumerate()
        .filter_map(|(pos, (_, e))| {
            let (x, y, w, h) = e.bounds?;
            // The application's own frame is never the control under a
            // point; leave such a hit to the live descent.
            (w > 0 && h > 0 && !crate::at_point_policy::is_top_level_shell_role(&e.role))
                .then_some((pos, x, y, w, h, native::is_passive_role(&e.role)))
        })
        .collect();
    let pos = native::select_click_target(&frames, sx, sy)?;
    let (idx, element) = indexed[pos];
    Some((*idx, element.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::element_token::{token_for, ResolvedElement};

    fn node(index: usize) -> AtspiNode {
        node_with_role(index, "button")
    }

    fn node_with_role(index: usize, role: &str) -> AtspiNode {
        AtspiNode {
            element_index: Some(index),
            role: role.into(),
            name: None,
            value: None,
            checked: None,
            enabled: None,
            selected: None,
            description: None,
            actions: Vec::new(),
            element_key: 999,
            identity: Some(AtspiIdentity {
                bus_name: ":1.1".into(),
                path: format!("/node/{index}"),
                frame_bus_name: ":1.1".into(),
                frame_path: "/frame".into(),
            }),
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
            object_ref: Some(ObjectRef {
                bus: ":1.1".into(),
                path: format!("/node/{index}"),
            }),
        }
    }

    #[test]
    fn sparse_application_indices_are_members_not_dense_offsets_or_native_keys() {
        let cache = ElementCache::new();
        let id = cache.publish(42, 7, CachedSnapshot::from_nodes(&[node(11), node(7)]));
        for index in [7, 11] {
            let resolved = cache
                .resolve_element_args(42, None, Some(&token_for(id, index)), None, None, "click")
                .unwrap();
            assert!(
                matches!(resolved, ResolvedElement::Element { element, .. } if element.identity.path == format!("/node/{index}"))
            );
        }
        for index in [0, 1, 8, 999] {
            assert!(cache
                .resolve_element_args(42, None, Some(&token_for(id, index)), None, None, "click")
                .is_err());
        }
    }

    #[test]
    fn duplicate_and_unindexed_nodes_do_not_create_members() {
        let mut unindexed = node(8);
        unindexed.element_index = None;
        let payload = CachedSnapshot::from_nodes(&[node(11), unindexed, node(7), node(11)]);
        assert_eq!(payload.len(), 2);
        assert_eq!(payload.retain(7).unwrap().identity.path, "/node/7");
        assert_eq!(payload.retain(11).unwrap().identity.path, "/node/11");
        assert!(payload.retain(8).is_none());
        assert!(payload.retain(0).is_none());
    }

    #[test]
    fn unproven_identity_is_discovery_only() {
        let mut unproven = node(3);
        unproven.identity = None;
        let payload = CachedSnapshot::from_nodes(&[unproven, node(4)]);
        assert_eq!(payload.len(), 1);
        assert!(payload.retain(3).is_none());
    }

    #[test]
    fn reordered_live_index_cannot_retarget_an_observed_control() {
        let cache = ElementCache::new();
        let observed = cache.publish(42, 7, CachedSnapshot::from_nodes(&[node(5)]));
        let mut replacement = node(5);
        replacement.identity.as_mut().unwrap().path = "/node/replacement".into();
        let current = cache.publish(42, 7, CachedSnapshot::from_nodes(&[replacement]));

        // The former observation is invalidated rather than resolving index 5
        // to the replacement. The current token retains the replacement's own
        // object address for the X11 click resolver to match directly.
        assert!(cache
            .resolve_element_args(42, None, Some(&token_for(observed, 5)), None, None, "click")
            .is_err());
        let current = cache
            .resolve_element_args(42, None, Some(&token_for(current, 5)), None, None, "click")
            .unwrap();
        assert!(
            matches!(current, ResolvedElement::Element { element, .. } if element.identity.path == "/node/replacement")
        );
    }

    #[test]
    fn replacement_retires_old_linux_membership() {
        let cache = ElementCache::new();
        let old = cache.publish(42, 7, CachedSnapshot::from_nodes(&[node(7), node(11)]));
        let fresh = cache.publish(42, 7, CachedSnapshot::from_nodes(&[node(3)]));
        for index in [7, 11] {
            let refusal = cache
                .resolve_element_args(42, None, Some(&token_for(old, index)), None, None, "click")
                .unwrap_err();
            assert_eq!(
                refusal.structured_content.unwrap()["refusal"]["code"],
                "stale_element_token"
            );
            assert!(cache
                .resolve_element_args(
                    42,
                    None,
                    Some(&token_for(fresh, index)),
                    None,
                    None,
                    "click"
                )
                .is_err());
        }
        let target = cache
            .resolve_element_args(42, None, Some(&token_for(fresh, 3)), None, None, "click")
            .unwrap();
        assert!(matches!(
            target,
            ResolvedElement::Element {
                window_id: Some(7),
                element,
                ..
            } if element.identity.path == "/node/3"
        ));
    }

    #[test]
    fn empty_linux_snapshot_has_no_element_zero() {
        let cache = ElementCache::new();
        let id = cache.publish(42, 7, CachedSnapshot::from_nodes(&[]));
        assert!(cache
            .resolve_element_args(42, None, Some(&token_for(id, 0)), None, None, "click")
            .is_err());
    }

    #[test]
    fn snapshot_hit_test_prefers_smallest_real_actuator_and_keeps_refs() {
        let pid = 424_242;
        let nodes = vec![
            node_with_role(0, "panel"),
            node_with_role(1, "push button"),
            node_with_role(2, "label"),
        ];
        let snapshot = update_snapshot(
            pid,
            77,
            &nodes,
            &[
                (0, 0, 0, 500, 500),
                (1, 100, 100, 50, 20),
                (2, 105, 102, 40, 16),
            ],
        );
        assert_eq!(snapshot.retain(1).unwrap().bounds, Some((100, 100, 50, 20)));
        let (idx, element) = hit_test(pid, 77, 110, 110).expect("hit");
        assert_eq!(idx, 1);
        assert_eq!(
            element.object_ref.as_ref().map(|r| r.path.as_str()),
            Some("/node/1")
        );
        assert!(hit_test(pid, 77, 900, 900).is_none());
        // Index lookup without an xid falls back to any snapshot for the pid.
        assert_eq!(cached_element(pid, None, 2).unwrap().role, "label");
        assert!(cached_element(pid, Some(78), 2).is_none());
        forget_window(pid, 77);
        assert!(cached_element(pid, None, 0).is_none());
    }
}
