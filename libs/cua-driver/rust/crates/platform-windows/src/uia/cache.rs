use super::UiaNode;
use cua_driver_core::element_token::{self, ElementTarget, ResolvedElement};
use cua_driver_core::protocol::ToolResult;
use windows::core::Interface;
use windows::Win32::UI::Accessibility::{IAccessible, IUIAutomationElement};

#[derive(Debug)]
pub struct RetainedElement {
    ptr: usize,
    pub kind: SnapshotKind,
    pub center: (i32, i32),
    pub rect: Option<(i32, i32, i32, i32)>,
    pub msaa_role: Option<i32>,
}

impl RetainedElement {
    pub fn as_ptr(&self) -> usize {
        self.ptr
    }

    pub fn is_uia(&self) -> bool {
        self.kind == SnapshotKind::Uia
    }

    pub fn focus_element(&self) -> anyhow::Result<()> {
        if !self.is_uia() {
            anyhow::bail!("element is an MSAA element, not a UIA element");
        }
        let element = unsafe { IUIAutomationElement::from_raw(self.ptr as *mut _) };
        let result = unsafe { element.SetFocus() };
        std::mem::forget(element);
        result.map_err(|e| anyhow::anyhow!("UIA SetFocus failed: {e}"))
    }

    pub fn element_has_keyboard_focus(&self) -> Option<bool> {
        if !self.is_uia() {
            return None;
        }
        let element = unsafe { IUIAutomationElement::from_raw(self.ptr as *mut _) };
        let focused = unsafe { element.CurrentHasKeyboardFocus() }
            .ok()
            .map(|value| value.as_bool());
        std::mem::forget(element);
        focused
    }
}

impl Clone for RetainedElement {
    fn clone(&self) -> Self {
        if self.ptr != 0 {
            unsafe {
                match self.kind {
                    SnapshotKind::Uia => {
                        let iface = IUIAutomationElement::from_raw(self.ptr as *mut _);
                        let dup = iface.clone();
                        std::mem::forget(iface);
                        std::mem::forget(dup);
                    }
                    SnapshotKind::Msaa => {
                        let iface = IAccessible::from_raw(self.ptr as *mut _);
                        let dup = iface.clone();
                        std::mem::forget(iface);
                        std::mem::forget(dup);
                    }
                }
            }
        }
        Self {
            ptr: self.ptr,
            kind: self.kind,
            center: self.center,
            rect: self.rect,
            msaa_role: self.msaa_role,
        }
    }
}

impl Drop for RetainedElement {
    fn drop(&mut self) {
        if self.ptr != 0 {
            unsafe {
                match self.kind {
                    SnapshotKind::Uia => drop(IUIAutomationElement::from_raw(self.ptr as *mut _)),
                    SnapshotKind::Msaa => drop(IAccessible::from_raw(self.ptr as *mut _)),
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotKind {
    Uia,
    Msaa,
}

pub struct CachedSnapshot {
    elements: Vec<RetainedElement>,
}

impl CachedSnapshot {
    pub fn from_nodes(nodes: &[UiaNode], kind: SnapshotKind) -> Self {
        Self {
            elements: nodes
                .iter()
                .filter(|node| node.element_index.is_some())
                .map(|node| RetainedElement {
                    ptr: node.element_ptr,
                    kind,
                    center: (node.center_x, node.center_y),
                    rect: node.rect,
                    msaa_role: node.msaa_role,
                })
                .collect(),
        }
    }
}

impl CachedSnapshot {
    fn retain_element(&self, index: usize) -> Option<RetainedElement> {
        self.elements
            .get(index)
            .filter(|element| element.ptr != 0)
            .cloned()
    }
}
pub fn identity_for_node(node: &UiaNode) -> Vec<u8> {
    serde_json::to_vec(&(
        &node.control_type,
        &node.name,
        &node.automation_id,
        &node.help_text,
        &node.actions,
        node.depth,
        node.in_web_content,
        node.msaa_role,
    ))
    .expect("UIA identity tuple")
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
        |w, t| resolve_fresh(w, t),
    )
}
fn resolve_fresh(w: u64, t: &ElementTarget) -> Result<Option<RetainedElement>, String> {
    let tree = super::walk_tree(w, None);
    let kind = if tree.nodes.iter().any(|n| n.msaa_role.is_some()) {
        SnapshotKind::Msaa
    } else {
        SnapshotKind::Uia
    };
    let payload = CachedSnapshot::from_nodes(&tree.nodes, kind);
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
