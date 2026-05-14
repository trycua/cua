//! UIA element cache for Windows.
//!
//! Mirrors the macOS ElementCache design: per-(pid, window_id) snapshots of
//! retained IUIAutomationElement COM pointers.
//!
//! Memory contract: UiaNode::element_ptr is a raw IUIAutomationElement vtable
//! pointer with an extra AddRef from clone()+forget() in the walker. Drop here
//! calls Release() to balance.

use super::UiaNode;
use std::collections::HashMap;
use std::sync::Mutex;
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::core::Interface;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub pid: u32,
    pub hwnd: u64,
}

pub struct CachedSnapshot {
    /// element_index → raw IUIAutomationElement pointer (retained).
    pub elements: Vec<usize>,
    /// element_index → screen-coordinate center (captured at walk time).
    pub centers: Vec<(i32, i32)>,
}

impl Drop for CachedSnapshot {
    fn drop(&mut self) {
        for ptr in &self.elements {
            if *ptr != 0 {
                // Reconstruct an interface reference and drop it (calls Release).
                unsafe {
                    let iface: IUIAutomationElement = IUIAutomationElement::from_raw(*ptr as *mut _);
                    drop(iface);
                }
            }
        }
    }
}

pub struct ElementCache {
    inner: Mutex<HashMap<CacheKey, CachedSnapshot>>,
}

impl ElementCache {
    pub fn new() -> Self {
        Self { inner: Mutex::new(HashMap::new()) }
    }

    pub fn update(&self, pid: u32, hwnd: u64, nodes: &[UiaNode]) {
        let actionable: Vec<&UiaNode> = nodes.iter().filter(|n| n.element_index.is_some()).collect();
        let elements: Vec<usize> = actionable.iter().map(|n| n.element_ptr).collect();
        let centers: Vec<(i32, i32)> = actionable.iter().map(|n| (n.center_x, n.center_y)).collect();
        let mut inner = self.inner.lock().unwrap();
        inner.insert(CacheKey { pid, hwnd }, CachedSnapshot { elements, centers });
    }

    pub fn get_element_ptr(&self, pid: u32, hwnd: u64, element_index: usize) -> Option<usize> {
        let inner = self.inner.lock().unwrap();
        inner.get(&CacheKey { pid, hwnd })?.elements.get(element_index).copied()
    }

    pub fn get_element_center(&self, pid: u32, hwnd: u64, element_index: usize) -> Option<(i32, i32)> {
        let inner = self.inner.lock().unwrap();
        inner.get(&CacheKey { pid, hwnd })?.centers.get(element_index).copied()
    }

    pub fn element_count(&self, pid: u32, hwnd: u64) -> usize {
        let inner = self.inner.lock().unwrap();
        inner.get(&CacheKey { pid, hwnd }).map(|s| s.elements.len()).unwrap_or(0)
    }
}

impl Default for ElementCache {
    fn default() -> Self { Self::new() }
}
