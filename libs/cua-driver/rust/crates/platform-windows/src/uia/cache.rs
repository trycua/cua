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
//! The locked-HashMap plumbing lives in `cua_driver_core::element_cache` — see
//! `docs/dedup-audit.md` item #3. This module owns the Windows-specific
//! `CacheKey`, `CachedSnapshot`, and the `Drop` impl that fires COM `Release`
//! when an entry is replaced or removed.

use super::UiaNode;
use cua_driver_core::element_cache::ElementCacheCore;
use windows::core::Interface;
use windows::Win32::UI::Accessibility::{IAccessible, IUIAutomationElement};

/// A cached element pointer borrowed out of the cache with an extra COM
/// `AddRef`, so it stays alive for the duration of a UIA/MSAA action even if a
/// concurrent `get_window_state` (→ [`ElementCache::update`]) replaces and
/// drops the snapshot it came from. Without this, the snapshot's `Drop` could
/// `Release` the element to zero while an in-flight click / SetValue was still
/// dereferencing the raw COM vtable pointer — a use-after-free. This is the
/// Windows analogue of the macOS AX-element crash fixed in #1796
/// (`EXC_BREAKPOINT` in `AXUIElementCopyActionNames`); here the same shape
/// would fault inside `IUIAutomation*::Invoke` / `ValuePattern::SetValue`.
///
/// The `AddRef` is taken **under the cache lock** (see
/// [`ElementCache::get_element_retained`]); the matching `Release` fires on
/// drop, via the `kind`-appropriate interface — mirroring `CachedSnapshot::drop`.
pub struct RetainedElement {
    ptr: usize,
    kind: SnapshotKind,
}

impl RetainedElement {
    /// The raw COM vtable pointer, valid for as long as this guard is held.
    pub fn as_ptr(&self) -> usize {
        self.ptr
    }

    /// True when the retained pointer is an `IUIAutomationElement` (the UIA
    /// path), not an MSAA `IAccessible`. `ScrollItemPattern::ScrollIntoView`
    /// only applies to UIA elements, so the scroll-into-view recovery gates on
    /// this before casting the pointer as a UIA interface.
    pub fn is_uia(&self) -> bool {
        matches!(self.kind, SnapshotKind::Uia)
    }
}

// `ptr` is a `usize` and `kind` is a plain `Copy` enum, so this is already
// `Send`; it's moved into / created inside `spawn_blocking` closures exactly
// like the bare `usize` it replaces. COM refcounting (`AddRef`/`Release`) is
// `InterlockedIncrement`-based and thread-safe, so the guard is safe to carry.

impl Drop for RetainedElement {
    fn drop(&mut self) {
        if self.ptr == 0 {
            return;
        }
        // Balance the AddRef taken in `get_element_retained`, releasing via the
        // same kind-appropriate vtable that `CachedSnapshot::drop` uses.
        unsafe {
            match self.kind {
                SnapshotKind::Uia => {
                    let iface: IUIAutomationElement =
                        IUIAutomationElement::from_raw(self.ptr as *mut _);
                    drop(iface);
                }
                SnapshotKind::Msaa => {
                    let iface: IAccessible = IAccessible::from_raw(self.ptr as *mut _);
                    drop(iface);
                }
            }
        }
    }
}

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

    /// Look up + COM-`AddRef` the element for `element_index` in (pid, hwnd),
    /// returning a guard that `Release`s on drop. The `AddRef` happens **under
    /// the cache lock**, so a concurrent [`update`](Self::update) (which
    /// replaces the snapshot and `Release`s its pointers in
    /// `CachedSnapshot::drop`) cannot free the element between the lookup and
    /// the `AddRef`. Hold the returned guard for the entire UIA/MSAA action —
    /// this is what makes element actions safe when two sessions drive the same
    /// `(pid, hwnd)`. Returns `None` if the index isn't cached.
    pub fn get_element_retained(
        &self,
        pid: u32,
        hwnd: u64,
        element_index: usize,
    ) -> Option<RetainedElement> {
        self.core
            .with_snapshot(&CacheKey { pid, hwnd }, |s| {
                let ptr = s.elements.get(element_index).copied()?;
                if ptr != 0 {
                    // Safety: still inside `with_snapshot`'s lock — the same
                    // `Mutex` that `insert` (the snapshot replace) takes — so
                    // the snapshot and thus this COM object is alive right now.
                    // Bump the refcount with the kind-appropriate vtable, using
                    // the walker's clone()+forget() idiom: `dup` is the extra
                    // ref the guard owns; the borrowed `iface` is forgotten so
                    // it doesn't Release the cache's own ref.
                    unsafe {
                        match s.kind {
                            SnapshotKind::Uia => {
                                let iface: IUIAutomationElement =
                                    IUIAutomationElement::from_raw(ptr as *mut _);
                                let dup = iface.clone(); // AddRef → +1
                                std::mem::forget(iface); // don't Release cache's ref
                                std::mem::forget(dup); // guard owns the +1
                            }
                            SnapshotKind::Msaa => {
                                let iface: IAccessible = IAccessible::from_raw(ptr as *mut _);
                                let dup = iface.clone();
                                std::mem::forget(iface);
                                std::mem::forget(dup);
                            }
                        }
                    }
                }
                Some(RetainedElement { ptr, kind: s.kind })
            })
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

#[cfg(test)]
#[path = "cache_uaf_repro.rs"]
mod cache_uaf_repro;

