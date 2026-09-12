use crate::element_cache::{register_runtime_cache, ElementCacheCore, SnapshotPayload};
use std::sync::Arc;

pub(crate) struct Payload(pub Vec<usize>);

impl SnapshotPayload for Payload {
    type Element = usize;
    fn len(&self) -> usize {
        self.0.len()
    }
    fn retain(&self, index: usize) -> Option<usize> {
        self.0.get(index).copied()
    }
}

pub(crate) fn cache() -> Arc<ElementCacheCore<Payload>> {
    let cache = Arc::new(ElementCacheCore::new());
    register_runtime_cache(&cache);
    cache
}
