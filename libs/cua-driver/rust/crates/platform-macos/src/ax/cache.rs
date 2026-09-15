use super::bindings::AXUIElementRef;
use super::tree::AXNode;
use core_foundation::base::{CFRelease, CFRetain, CFTypeRef};
use cua_driver_core::element_cache::{ElementCacheCore, SnapshotPayload};

pub struct RetainedElement(usize);

impl RetainedElement {
    pub fn as_ptr(&self) -> usize {
        self.0
    }

    pub unsafe fn retain(ptr: usize) -> Self {
        if ptr != 0 {
            unsafe { CFRetain(ptr as AXUIElementRef as CFTypeRef) };
        }
        Self(ptr)
    }
}

impl Clone for RetainedElement {
    fn clone(&self) -> Self {
        unsafe { Self::retain(self.0) }
    }
}

impl Drop for RetainedElement {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { CFRelease(self.0 as AXUIElementRef as CFTypeRef) };
        }
    }
}

pub struct CachedSnapshot {
    pub elements: Vec<usize>,
}

impl CachedSnapshot {
    pub fn from_nodes(nodes: &[AXNode]) -> Self {
        Self {
            elements: nodes
                .iter()
                .filter(|node| node.element_index.is_some())
                .map(|node| node.element_ptr)
                .collect(),
        }
    }
}

impl CachedSnapshot {
    fn retain_element(&self, index: usize) -> Option<RetainedElement> {
        self.elements
            .get(index)
            .filter(|ptr| **ptr != 0)
            .map(|ptr| unsafe { RetainedElement::retain(*ptr) })
    }
}
impl SnapshotPayload for CachedSnapshot {
    type Element = RetainedElement;
    fn len(&self) -> usize {
        self.elements.len()
    }
    fn retain(&self, index: usize) -> Option<Self::Element> {
        self.retain_element(index)
    }
    fn resolve_fresh(
        pid: i32,
        window_id: u64,
        index: usize,
    ) -> Result<Option<Self::Element>, String> {
        let window_id = u32::try_from(window_id)
            .map_err(|_| format!("window_id {window_id} is not a valid macOS window id"))?;
        let tree = super::tree::walk_tree(pid, Some(window_id), None);
        let payload = Self::from_nodes(&tree.nodes);
        Ok(payload.retain_element(index))
    }
}

impl Drop for CachedSnapshot {
    fn drop(&mut self) {
        for ptr in &self.elements {
            if *ptr != 0 {
                unsafe { CFRelease(*ptr as AXUIElementRef as CFTypeRef) };
            }
        }
    }
}

pub type ElementCache = ElementCacheCore<CachedSnapshot>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_projection_has_no_element() {
        assert!(CachedSnapshot::from_nodes(&[]).retain_element(0).is_none());
    }
}
