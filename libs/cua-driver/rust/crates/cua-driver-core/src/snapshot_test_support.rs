use crate::snapshot_store::{register_runtime_store, SnapshotPayload, SnapshotStore};
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

pub(crate) fn cache() -> Arc<SnapshotStore<Payload>> {
    let cache = Arc::new(SnapshotStore::new());
    register_runtime_store(&cache);
    cache
}

/// The first text block of a tool result.
pub(crate) fn text(result: &crate::protocol::ToolResult) -> String {
    result
        .content
        .iter()
        .find_map(|content| match content {
            crate::protocol::Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}
