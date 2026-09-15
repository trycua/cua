use super::bindings::AXUIElementRef;
use super::tree::AXNode;
use core_foundation::base::{CFRelease, CFRetain, CFTypeRef};
use cua_driver_core::element_token::{self, ElementTarget, ResolvedElement};
use cua_driver_core::protocol::ToolResult;
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
pub struct FreshAxElements {
    pub elements: Vec<usize>,
}
impl FreshAxElements {
    pub fn from_nodes(nodes: &[AXNode]) -> Self {
        Self {
            elements: nodes
                .iter()
                .filter(|n| n.element_index.is_some())
                .map(|n| n.element_ptr)
                .collect(),
        }
    }
    fn retain_element(&self, index: usize) -> Option<RetainedElement> {
        self.elements
            .get(index)
            .filter(|p| **p != 0)
            .map(|p| unsafe { RetainedElement::retain(*p) })
    }
}
impl Drop for FreshAxElements {
    fn drop(&mut self) {
        for ptr in &self.elements {
            if *ptr != 0 {
                unsafe { CFRelease(*ptr as AXUIElementRef as CFTypeRef) };
            }
        }
    }
}
pub fn identity_for_node(n: &AXNode) -> Vec<u8> {
    serde_json::to_vec(&(
        &n.role,
        &n.title,
        &n.description,
        &n.identifier,
        &n.actions,
        n.depth,
        n.in_web_content,
    ))
    .expect("AX identity tuple")
}
pub fn resolve_element_args(
    pid: i32,
    element_index: Option<usize>,
    element_token: Option<&str>,
    snapshot_id: Option<&str>,
    window_id: Option<u64>,
    tool: &str,
) -> Result<ResolvedElement<RetainedElement>, ToolResult> {
    element_token::resolve_element_args(
        pid,
        element_index,
        element_token,
        snapshot_id,
        window_id,
        tool,
        |w, t| resolve_fresh(pid, w, t),
    )
}
fn resolve_fresh(pid: i32, w: u64, t: &ElementTarget) -> Result<Option<RetainedElement>, String> {
    let w =
        u32::try_from(w).map_err(|_| format!("window_id {w} is not a valid macOS window id"))?;
    let tree = super::tree::walk_tree(pid, Some(w), None);
    let payload = FreshAxElements::from_nodes(&tree.nodes);
    let matched = if !t.has_identity() {
        tree.nodes
            .iter()
            .find(|n| n.element_index == Some(t.element_index))
            .and_then(|n| n.element_index)
    } else {
        let mut m = tree.nodes.iter().filter_map(|n| {
            n.element_index
                .filter(|_| t.matches_identity(&identity_for_node(n)))
        });
        let f = m.next();
        f.filter(|_| m.next().is_none())
    };
    Ok(matched.and_then(|i| payload.retain_element(i)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_projection_has_no_element() {
        assert!(FreshAxElements::from_nodes(&[]).retain_element(0).is_none());
    }
}
