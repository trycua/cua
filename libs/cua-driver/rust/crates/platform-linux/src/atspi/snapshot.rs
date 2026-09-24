use super::{AtspiIdentity, AtspiNode};
use cua_driver_core::snapshot_store::{SnapshotPayload, SnapshotStore};
use std::collections::HashMap;

pub struct AtspiSnapshot {
    elements: HashMap<usize, AtspiIdentity>,
}

impl AtspiSnapshot {
    pub fn from_nodes(nodes: &[AtspiNode]) -> Self {
        let elements = nodes
            .iter()
            .filter_map(|node| Some((node.element_index?, node.identity.clone()?)))
            .collect();
        Self { elements }
    }
}

impl SnapshotPayload for AtspiSnapshot {
    type Element = AtspiIdentity;
    fn len(&self) -> usize {
        self.elements.len()
    }
    fn retain(&self, index: usize) -> Option<AtspiIdentity> {
        self.elements.get(&index).cloned()
    }
}

pub type Snapshots = SnapshotStore<AtspiSnapshot>;

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
            identity: Some(AtspiIdentity {
                bus_name: ":1.1".into(),
                path: format!("/node/{index}"),
                frame_bus_name: ":1.1".into(),
                frame_path: "/frame".into(),
            }),
            depth: 0,
            parent_element_index: None,
            in_web_content: false,
        }
    }

    #[test]
    fn sparse_application_indices_are_members_not_dense_offsets_or_native_keys() {
        let cache = Snapshots::new();
        let id = cache.publish(42, 7, AtspiSnapshot::from_nodes(&[node(11), node(7)]));
        for index in [7, 11] {
            let resolved = cache
                .resolve(
                    42,
                    &serde_json::json!({ "element_token": token_for(id, index) }),
                )
                .unwrap();
            assert!(
                matches!(resolved, ResolvedElement::Element { element, .. } if element.path == format!("/node/{index}"))
            );
        }
        for index in [0, 1, 8, 999] {
            assert!(cache
                .resolve(
                    42,
                    &serde_json::json!({ "element_token": token_for(id, index) })
                )
                .is_err());
        }
    }

    #[test]
    fn duplicate_and_unindexed_nodes_do_not_create_members() {
        let mut unindexed = node(8);
        unindexed.element_index = None;
        let payload = AtspiSnapshot::from_nodes(&[node(11), unindexed, node(7), node(11)]);
        assert_eq!(payload.len(), 2);
        assert_eq!(payload.retain(7).unwrap().path, "/node/7");
        assert_eq!(payload.retain(11).unwrap().path, "/node/11");
        assert_eq!(payload.retain(8), None);
        assert_eq!(payload.retain(0), None);
    }

    #[test]
    fn reordered_live_index_cannot_retarget_an_observed_control() {
        let cache = Snapshots::new();
        let observed = cache.publish(42, 7, AtspiSnapshot::from_nodes(&[node(5)]));
        let mut replacement = node(5);
        replacement.identity.as_mut().unwrap().path = "/node/replacement".into();
        let current = cache.publish(42, 7, AtspiSnapshot::from_nodes(&[replacement]));

        // The former observation is invalidated rather than resolving index 5
        // to the replacement. The current token retains the replacement's own
        // object address for the X11 click resolver to match directly.
        assert!(cache
            .resolve(
                42,
                &serde_json::json!({ "element_token": token_for(observed, 5) })
            )
            .is_err());
        let current = cache
            .resolve(
                42,
                &serde_json::json!({ "element_token": token_for(current, 5) }),
            )
            .unwrap();
        assert!(
            matches!(current, ResolvedElement::Element { element, .. } if element.path == "/node/replacement")
        );
    }

    #[test]
    fn replacement_retires_old_linux_membership() {
        let cache = Snapshots::new();
        let old = cache.publish(42, 7, AtspiSnapshot::from_nodes(&[node(7), node(11)]));
        let fresh = cache.publish(42, 7, AtspiSnapshot::from_nodes(&[node(3)]));
        for index in [7, 11] {
            let refusal = cache
                .resolve(
                    42,
                    &serde_json::json!({ "element_token": token_for(old, index) }),
                )
                .unwrap_err();
            assert_eq!(
                refusal.structured_content.unwrap()["refusal"]["code"],
                "stale_element_token"
            );
            assert!(cache
                .resolve(
                    42,
                    &serde_json::json!({ "element_token": token_for(fresh, index) })
                )
                .is_err());
        }
        let target = cache
            .resolve(
                42,
                &serde_json::json!({ "element_token": token_for(fresh, 3) }),
            )
            .unwrap();
        assert!(matches!(
            target,
            ResolvedElement::Element {
                window_id: 7,
                element,
                ..
            } if element.path == "/node/3"
        ));
    }

    #[test]
    fn compositor_window_ids_do_not_alias_their_low_bits() {
        let cache = Snapshots::new();
        let window = (1_u64 << 40) | 7;
        let low = cache.publish(42, 7, AtspiSnapshot::from_nodes(&[node(7)]));
        let high = cache.publish(42, window, AtspiSnapshot::from_nodes(&[node(11)]));
        let target = cache
            .resolve(
                42,
                &serde_json::json!({ "element_token": token_for(high, 11) }),
            )
            .unwrap();
        assert!(
            matches!(target, ResolvedElement::Element { window_id, element, .. } if window_id == window && element.path == "/node/11")
        );
        cache.remove(42, window);
        assert!(matches!(
            cache
                .resolve(
                    42,
                    &serde_json::json!({ "element_token": token_for(low, 7) })
                )
                .unwrap(),
            ResolvedElement::Element { window_id: 7, .. }
        ));
    }

    #[test]
    fn empty_linux_snapshot_has_no_element_zero() {
        let cache = Snapshots::new();
        let id = cache.publish(42, 7, AtspiSnapshot::from_nodes(&[]));
        assert!(cache
            .resolve(
                42,
                &serde_json::json!({ "element_token": token_for(id, 0) })
            )
            .is_err());
    }
}
