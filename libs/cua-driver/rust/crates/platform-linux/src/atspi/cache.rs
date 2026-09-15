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
    fn resolve_fresh(pid: i32, window_id: u64, index: usize) -> Result<Option<usize>, String> {
        let tree = super::walk_tree(pid as u32, window_id, None);
        if !tree.trusted {
            return Err("current accessibility state is unavailable from AT-SPI".into());
        }
        if !tree.window_scoped {
            return Err(format!(
                "current accessibility state is not scoped to window_id {window_id}"
            ));
        }
        Ok(tree
            .nodes
            .iter()
            .filter_map(|node| node.element_index)
            .any(|current| current == index)
            .then_some(index))
    }
}

pub type ElementCache = ElementCacheCore<CachedSnapshot>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_projection_has_no_members() {
        let payload = CachedSnapshot::from_nodes(&[]);
        assert_eq!(payload.len(), 0);
        assert_eq!(payload.retain(0), None);
    }
}
