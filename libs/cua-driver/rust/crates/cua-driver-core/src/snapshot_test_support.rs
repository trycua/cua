// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use crate::snapshot_store::{register_runtime_store, SnapshotPayload, SnapshotRows, SnapshotStore};
use crate::window_state_view::{ReadRows, ViewKey, DEFAULT_MAX_ELEMENTS};
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

/// The view key of a default `get_window_state` read.
pub(crate) fn default_view() -> ViewKey {
    ViewKey {
        query: None,
        max_elements: DEFAULT_MAX_ELEMENTS,
        max_depth: None,
    }
}

/// Publish a read of `markdown` as `get_window_state` does: a snapshot of the
/// window, with the rows its read rendered kept on it.
pub(crate) fn publish_read(
    store: &SnapshotStore<Payload>,
    pid: i32,
    window_id: u64,
    markdown: &str,
) -> u32 {
    let id = store.publish(pid, window_id, Payload(vec![0]));
    store.keep_rows(
        id,
        ReadRows {
            key: default_view(),
            markdown: markdown.to_owned(),
        },
    );
    id
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
