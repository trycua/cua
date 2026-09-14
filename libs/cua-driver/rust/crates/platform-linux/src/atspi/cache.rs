use super::AtspiNode;
use cua_driver_core::element_cache::{ElementCacheCore, SnapshotPayload};

pub struct CachedSnapshot {
    elements: Vec<usize>,
}

impl CachedSnapshot {
    pub fn from_nodes(nodes: &[AtspiNode]) -> Self {
        let mut elements: Vec<_> = nodes.iter().filter_map(|node| node.element_index).collect();
        elements.sort_unstable();
        elements.dedup();
        Self { elements }
    }
}

impl SnapshotPayload for CachedSnapshot {
    type Element = usize;
    fn len(&self) -> usize {
        self.elements.len()
    }
    fn retain(&self, index: usize) -> Option<usize> {
        self.elements.binary_search(&index).ok().map(|_| index)
    }
}

pub type ElementCache = ElementCacheCore<CachedSnapshot>;

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::element_token::{token_for, ResolvedElement};

    fn node(index: usize) -> AtspiNode {
        AtspiNode {
            element_index: Some(index),
            role: "button".into(),
            name: None,
            value: None,
            checked: None,
            enabled: None,
            selected: None,
            description: None,
            actions: Vec::new(),
            element_key: 999,
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
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
                matches!(resolved, ResolvedElement::Element { element, .. } if element == index)
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
        assert_eq!(payload.retain(7), Some(7));
        assert_eq!(payload.retain(11), Some(11));
        assert_eq!(payload.retain(8), None);
        assert_eq!(payload.retain(0), None);
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
                element: 3,
                ..
            }
        ));
    }

    #[test]
    fn compositor_window_ids_do_not_alias_their_low_bits() {
        let cache = ElementCache::new();
        let window = (1_u64 << 40) | 7;
        let low = cache.publish(42, 7, CachedSnapshot::from_nodes(&[node(7)]));
        let high = cache.publish(42, window, CachedSnapshot::from_nodes(&[node(11)]));
        let target = cache
            .resolve_element_args(
                42,
                None,
                Some(&token_for(high, 11)),
                None,
                Some(window),
                "click",
            )
            .unwrap();
        assert!(
            matches!(target, ResolvedElement::Element { window_id: Some(id), element: 11, .. } if id == window)
        );
        assert!(cache
            .resolve_element_args(42, None, Some(&token_for(high, 11)), None, Some(7), "click")
            .is_err());
        cache.remove(42, window);
        assert!(cache
            .resolve_element_args(42, None, Some(&token_for(low, 7)), None, Some(7), "click")
            .is_ok());
    }

    #[test]
    fn empty_linux_snapshot_has_no_element_zero() {
        let cache = ElementCache::new();
        let id = cache.publish(42, 7, CachedSnapshot::from_nodes(&[]));
        assert!(cache
            .resolve_element_args(42, None, Some(&token_for(id, 0)), None, None, "click")
            .is_err());
    }
}
