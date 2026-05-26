//! Accessibility element cache for Windows.
//!
//! Mirrors the macOS ElementCache design: per-(pid, window_id) snapshots of
//! retained COM pointers (either `IUIAutomationElement` for the UIA primary
//! path, or `IAccessible` for the MSAA fallback used on SAL/VCL targets).
//!
//! Memory contract: `UiaNode::element_ptr` is a raw COM vtable pointer with
//! an extra AddRef from `clone()+forget()` in the walker. `CachedSnapshot::Drop`
//! calls `Release()` via the kind-appropriate vtable to balance.
//!
//! The locked-HashMap plumbing lives in `mcp_server::element_cache` — see
//! `docs/dedup-audit.md` item #3. This module owns the Windows-specific
//! `CacheKey`, `CachedSnapshot`, and the `Drop` impl that fires COM `Release`
//! when an entry is replaced or removed.

use super::UiaNode;
use mcp_server::element_cache::ElementCacheCore;
use windows::core::Interface;
use windows::Win32::UI::Accessibility::{IAccessible, IUIAutomationElement};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub pid: u32,
    pub hwnd: u64,
}

/// Which COM interface the cached pointers reference. Determines the
/// `from_raw` cast `Drop` performs and the dispatch path the click tool
/// takes when an element is invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotKind {
    /// `element_ptr` values are `IUIAutomationElement` pointers; click
    /// tool tries Invoke/Toggle/SelectionItem/ExpandCollapse patterns.
    Uia,
    /// `element_ptr` values are `IAccessible` pointers (MSAA fallback for
    /// SAL/VCL classes); click tool dispatches via `accDoDefaultAction`
    /// for invoke, right-edge SendInput for expand.
    Msaa,
}

pub struct CachedSnapshot {
    pub kind: SnapshotKind,
    /// element_index → raw COM pointer (retained).
    pub elements: Vec<usize>,
    /// element_index → screen-coordinate center (captured at walk time).
    pub centers: Vec<(i32, i32)>,
    /// element_index → screen-coordinate rect (l, t, r, b). `None` when
    /// the element didn't report a meaningful bounding box.
    pub rects: Vec<Option<(i32, i32, i32, i32)>>,
    /// element_index → MSAA role code (`Some` for MSAA-walked entries,
    /// `None` for UIA-walked entries). The click tool reads this to gate
    /// right-edge dispatch on `ROLE_SYSTEM_BUTTONDROPDOWN` etc.
    pub msaa_roles: Vec<Option<i32>>,
}

impl Drop for CachedSnapshot {
    fn drop(&mut self) {
        for ptr in &self.elements {
            if *ptr == 0 {
                continue;
            }
            unsafe {
                match self.kind {
                    SnapshotKind::Uia => {
                        let iface: IUIAutomationElement =
                            IUIAutomationElement::from_raw(*ptr as *mut _);
                        drop(iface);
                    }
                    SnapshotKind::Msaa => {
                        let iface: IAccessible = IAccessible::from_raw(*ptr as *mut _);
                        drop(iface);
                    }
                }
            }
        }
    }
}

pub struct ElementCache {
    core: ElementCacheCore<CacheKey, CachedSnapshot>,
}

impl ElementCache {
    pub fn new() -> Self {
        Self {
            core: ElementCacheCore::new(),
        }
    }

    /// Update the snapshot for (pid, hwnd) with the actionable elements
    /// from a UIA walk. Mirrors `update_msaa` for the MSAA path.
    pub fn update(&self, pid: u32, hwnd: u64, nodes: &[UiaNode]) {
        self.update_with_kind(pid, hwnd, nodes, SnapshotKind::Uia);
    }

    /// Same as `update` but tags the snapshot as MSAA so Drop releases the
    /// pointers as `IAccessible` and the click tool routes through the
    /// MSAA dispatch path.
    pub fn update_msaa(&self, pid: u32, hwnd: u64, nodes: &[UiaNode]) {
        self.update_with_kind(pid, hwnd, nodes, SnapshotKind::Msaa);
    }

    fn update_with_kind(&self, pid: u32, hwnd: u64, nodes: &[UiaNode], kind: SnapshotKind) {
        let actionable: Vec<&UiaNode> = nodes.iter().filter(|n| n.element_index.is_some()).collect();
        let elements: Vec<usize> = actionable.iter().map(|n| n.element_ptr).collect();
        let centers: Vec<(i32, i32)> = actionable.iter().map(|n| (n.center_x, n.center_y)).collect();
        let rects: Vec<Option<(i32, i32, i32, i32)>> = actionable.iter().map(|n| n.rect).collect();
        let msaa_roles: Vec<Option<i32>> = actionable.iter().map(|n| n.msaa_role).collect();
        self.core.insert(
            CacheKey { pid, hwnd },
            CachedSnapshot { kind, elements, centers, rects, msaa_roles },
        );
    }

    pub fn get_element_ptr(&self, pid: u32, hwnd: u64, element_index: usize) -> Option<usize> {
        self.core
            .with_snapshot(&CacheKey { pid, hwnd }, |s| s.elements.get(element_index).copied())
            .flatten()
    }

    pub fn get_element_center(&self, pid: u32, hwnd: u64, element_index: usize) -> Option<(i32, i32)> {
        self.core
            .with_snapshot(&CacheKey { pid, hwnd }, |s| s.centers.get(element_index).copied())
            .flatten()
    }

    /// Cached screen rect for the element. Used by the click tool to
    /// compute the right-edge dispatch point for `action:"expand"` on
    /// MSAA BUTTONDROPDOWN.
    pub fn get_element_rect(&self, pid: u32, hwnd: u64, element_index: usize) -> Option<(i32, i32, i32, i32)> {
        self.core
            .with_snapshot(&CacheKey { pid, hwnd }, |s| s.rects.get(element_index).copied().flatten())
            .flatten()
    }

    /// Kind + MSAA role for the element. `(Msaa, Some(0x38))` identifies a
    /// MSAA BUTTONDROPDOWN; click tool routes `action:"expand"` to
    /// right-edge SendInput for these.
    pub fn get_element_kind_and_role(
        &self,
        pid: u32,
        hwnd: u64,
        element_index: usize,
    ) -> Option<(SnapshotKind, Option<i32>)> {
        self.core
            .with_snapshot(&CacheKey { pid, hwnd }, |s| {
                let role = s.msaa_roles.get(element_index).copied().flatten();
                Some((s.kind, role))
            })
            .flatten()
    }

    pub fn element_count(&self, pid: u32, hwnd: u64) -> usize {
        self.core
            .with_snapshot(&CacheKey { pid, hwnd }, |s| s.elements.len())
            .unwrap_or(0)
    }
}

impl Default for ElementCache {
    fn default() -> Self {
        Self::new()
    }
}
