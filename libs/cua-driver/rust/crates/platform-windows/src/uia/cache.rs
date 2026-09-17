use super::UiaNode;
use cua_driver_core::element_cache::{ElementCacheCore, SnapshotPayload};
use windows::core::Interface;
use windows::Win32::UI::Accessibility::{IAccessible, IUIAutomationElement};

pub type ElementCache = ElementCacheCore<CachedSnapshot>;

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

impl SnapshotPayload for CachedSnapshot {
    type Element = RetainedElement;

    fn len(&self) -> usize {
        self.elements.len()
    }

    fn retain(&self, index: usize) -> Option<Self::Element> {
        self.elements
            .get(index)
            .filter(|element| element.ptr != 0)
            .cloned()
    }
}

#[cfg(test)]
#[path = "cache_uaf_repro.rs"]
mod cache_uaf_repro;
