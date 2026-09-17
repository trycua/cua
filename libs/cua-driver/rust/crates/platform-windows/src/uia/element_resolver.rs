use super::UiaNode;
use cua_driver_core::element_token::{self, ElementTarget, ResolvedElement};
use cua_driver_core::protocol::ToolResult;
use windows::core::Interface;
use windows::Win32::UI::Accessibility::{IAccessible, IUIAutomationElement};

#[derive(Debug)]
pub struct RetainedElement {
    ptr: usize,
    pub kind: ElementBackend,
    pub center: (i32, i32),
    pub rect: Option<(i32, i32, i32, i32)>,
    pub msaa_role: Option<i32>,
}

impl RetainedElement {
    pub fn as_ptr(&self) -> usize {
        self.ptr
    }

    pub fn is_uia(&self) -> bool {
        self.kind == ElementBackend::Uia
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
                    ElementBackend::Uia => {
                        let iface = IUIAutomationElement::from_raw(self.ptr as *mut _);
                        let dup = iface.clone();
                        std::mem::forget(iface);
                        std::mem::forget(dup);
                    }
                    ElementBackend::Msaa => {
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
                    ElementBackend::Uia => drop(IUIAutomationElement::from_raw(self.ptr as *mut _)),
                    ElementBackend::Msaa => drop(IAccessible::from_raw(self.ptr as *mut _)),
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementBackend {
    Uia,
    Msaa,
}

pub struct FreshUiaElements {
    elements: Vec<RetainedElement>,
}

impl FreshUiaElements {
    pub fn from_nodes(nodes: &[UiaNode], kind: ElementBackend) -> Self {
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

impl FreshUiaElements {
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
pub async fn resolve_element_args(
    pid: i32,
    element_index: Option<usize>,
    element_token: Option<&str>,
    snapshot_id: Option<&str>,
    window_id: Option<u64>,
    tool: &str,
) -> Result<ResolvedElement<RetainedElement>, ToolResult> {
    element_token::resolve_native(
        pid,
        element_index,
        element_token,
        snapshot_id,
        window_id,
        tool,
        move |w, t| resolve_fresh(pid, w, t),
    )
    .await
}
pub(crate) fn resolve_fresh(
    pid: i32,
    w: u64,
    t: &ElementTarget,
) -> Result<Option<RetainedElement>, String> {
    let mut owner = 0;
    let thread = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            windows::Win32::Foundation::HWND(w as *mut _),
            Some(&mut owner),
        )
    };
    if thread == 0 || owner != pid as u32 {
        return Err(format!("window_id {w} does not belong to pid {pid}"));
    }
    let tree = super::walk_tree(w, None);
    let kind = if tree.nodes.iter().any(|n| n.msaa_role.is_some()) {
        ElementBackend::Msaa
    } else {
        ElementBackend::Uia
    };
    let payload = FreshUiaElements::from_nodes(&tree.nodes, kind);
    let matched = t.resolve_unique(
        tree.nodes.iter().filter_map(|node| {
            node.element_index
                .map(|index| (identity_for_node(node), index))
        }),
        tree.complete,
    )?;
    Ok(matched.and_then(|i| payload.retain_element(i)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fresh_lookup_refuses_a_window_owned_by_another_process() {
        let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow() };
        let mut owner = 0;
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
                hwnd,
                Some(&mut owner),
            );
        }
        let pid = std::process::id() as i32;
        assert_ne!(owner, pid as u32);
        let window = hwnd.0 as usize as u64;
        let snapshot = element_token::mint_snapshot_handle(pid, window);
        let token = element_token::token_for_identity(&snapshot, 0, b"control").unwrap();
        let error = resolve_element_args(pid, None, Some(&token), None, None, "click")
            .await
            .unwrap_err();
        assert_eq!(
            error.structured_content.unwrap()["refusal"]["message"],
            format!("window_id {window} does not belong to pid {pid}")
        );
    }
}
