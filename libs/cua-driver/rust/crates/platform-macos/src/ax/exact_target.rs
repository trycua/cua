//! Fresh exact-target acquisition for background input decisions.
//!
//! Gathers the facts [`cua_driver_core::background_input`] needs about one
//! requested `(pid, CGWindowID)` immediately before a background mutation:
//! WindowServer ownership, fresh `AXWindows` membership (mapped through
//! `_AXUIElementGetWindow`), minimized/hidden state, competing same-pid AX
//! top-level keyboard destinations, and addressed-element ancestry. All reads are
//! bounded and fail closed — an unreadable fact never unlocks a route.

use core_foundation::base::{CFRelease, CFTypeRef};
use cua_driver_core::background_input::{
    BackgroundTargetFacts, ElementAncestry, WindowServerOwnership,
};

use super::bindings::{
    ax_get_window_id, copy_ax_windows, copy_bool_attr, copy_element_attr, copy_string_attr,
    focused_element_of_pid, AXUIElementCreateApplication, AXUIElementRef,
};
use crate::windows::{all_windows, resolve_window_owner, WindowOwner};

/// Maximum number of nodes inspected in an `AXParent` ascent.
const MAX_ANCESTRY_DEPTH: usize = 40;

/// The live adapter and synthetic graphs exercise the same bounded walk.
trait AncestryElement: Sized {
    fn window(&self) -> Option<Self>;
    fn window_id(&self) -> Option<u32>;
    fn role(&self) -> Option<String>;
    fn parent(&self) -> Option<Self>;
}

struct AxAncestryElement {
    ptr: AXUIElementRef,
    owned: bool,
}

impl AxAncestryElement {
    fn copy_attribute(&self, attribute: &str) -> Option<Self> {
        // SAFETY: the initial node is borrowed for this synchronous walk;
        // attribute nodes own the retain returned by copy_element_attr.
        unsafe { copy_element_attr(self.ptr, attribute) }.map(|ptr| Self { ptr, owned: true })
    }
}

impl Drop for AxAncestryElement {
    fn drop(&mut self) {
        if self.owned {
            unsafe { CFRelease(self.ptr as CFTypeRef) };
        }
    }
}

impl AncestryElement for AxAncestryElement {
    fn window(&self) -> Option<Self> {
        self.copy_attribute("AXWindow")
    }

    fn window_id(&self) -> Option<u32> {
        unsafe { ax_get_window_id(self.ptr) }
    }

    fn role(&self) -> Option<String> {
        unsafe { copy_string_attr(self.ptr, "AXRole") }
    }

    fn parent(&self) -> Option<Self> {
        self.copy_attribute("AXParent")
    }
}

/// Prove whether `element` belongs to `target_window_id`.
///
/// Both the direct `AXWindow` attribute and every window-like node in a bounded
/// `AXParent` ascent are considered. This matters for AppKit sheets: descendants
/// can first map to the sheet's separate compositor window, then ascend through
/// `AXSheet` to the parent `AXWindow`. A different mapped identity is evidence
/// against the target only after the complete chain has been checked.
///
/// # Safety
///
/// `element` must be a valid `AXUIElementRef` for the duration of the call.
pub unsafe fn element_window_ancestry(
    element: AXUIElementRef,
    target_window_id: u32,
) -> ElementAncestry {
    walk_window_ancestry(
        AxAncestryElement {
            ptr: element,
            owned: false,
        },
        target_window_id,
    )
}

fn walk_window_ancestry(element: impl AncestryElement, target_window_id: u32) -> ElementAncestry {
    let mut saw_other_window = false;
    if let Some(window) = element.window() {
        let window_id = window.window_id();
        if window_id == Some(target_window_id) {
            return ElementAncestry::ProvenDescendant;
        }
        saw_other_window = window_id.is_some();
    }

    let mut current = element;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        let role = current.role();
        if matches!(role.as_deref(), Some("AXWindow" | "AXSheet")) {
            match current.window_id() {
                Some(window_id) if window_id == target_window_id => {
                    return ElementAncestry::ProvenDescendant;
                }
                Some(_) => saw_other_window = true,
                None => {}
            }
        }
        if matches!(role.as_deref(), Some("AXApplication") | None) {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent;
    }

    if saw_other_window {
        ElementAncestry::OutsideTargetWindow
    } else {
        ElementAncestry::Unproven
    }
}

/// The process's focused AX element, but only when it provably belongs to the
/// requested window. Returns a retained element the caller must release.
///
/// This is the only focused-element reader background window-scoped keyboard
/// paths may use: a PID-global focused element can belong to a sibling window,
/// and sibling state must never address or confirm the requested target.
///
/// # Safety
///
/// Caller must `CFRelease` the returned element.
pub unsafe fn focused_element_in_window(pid: i32, window_id: u32) -> Option<AXUIElementRef> {
    let element = focused_element_of_pid(pid)?;
    if element_window_ancestry(element, window_id) == ElementAncestry::ProvenDescendant {
        Some(element)
    } else {
        CFRelease(element as CFTypeRef);
        None
    }
}

/// One fresh `AXWindows` row: the mapped CGWindowID plus its minimized state.
/// `minimized: None` means the attribute could not be read — unknown, not
/// "not minimized".
struct AxWindowRecord {
    window_id: u32,
    minimized: Option<bool>,
}

/// Map the application's fresh `AXWindows` through `_AXUIElementGetWindow`.
/// Windows whose id the SPI cannot resolve are omitted: an unmappable window
/// can never satisfy an exact-target requirement.
unsafe fn ax_window_records(app: AXUIElementRef) -> Vec<AxWindowRecord> {
    copy_ax_windows(app)
        .into_iter()
        .filter_map(|window| {
            let record = ax_get_window_id(window).map(|window_id| AxWindowRecord {
                window_id,
                minimized: copy_bool_attr(window, "AXMinimized"),
            });
            CFRelease(window as CFTypeRef);
            record
        })
        .collect()
}

/// Count independently AX-mapped, non-minimized sibling top-level windows.
///
/// WindowServer may expose several layer-0 compositor surfaces for one native
/// Electron, Tauri, or WebKit window. A raw same-pid CGWindow row is therefore
/// not enough to prove another process-scoped keyboard destination. Requiring a
/// fresh `AXWindows` mapping preserves the fail-closed two-window guard while
/// ignoring render surfaces that cannot independently become the AX key window.
fn count_competing_keyboard_destinations(
    pid: i32,
    target_window_id: u32,
    window_server_rows: impl IntoIterator<Item = (i32, u32)>,
    ax_records: &[AxWindowRecord],
) -> usize {
    window_server_rows
        .into_iter()
        .filter(|(owner_pid, window_id)| {
            *owner_pid == pid
                && *window_id != target_window_id
                && ax_records
                    .iter()
                    .any(|record| record.window_id == *window_id && record.minimized != Some(true))
        })
        .count()
}

/// Gather fresh background-input facts for one `(pid, window_id)` target.
///
/// `element_ptr` is an optional retained `AXUIElementRef` (as `usize`) for an
/// explicitly addressed element; the caller must keep it retained for the
/// duration of this call. Blocking: performs one CGWindowList enumeration and
/// bounded AX reads. Call from a blocking context immediately before deciding.
pub fn gather_background_facts(
    pid: i32,
    window_id: u32,
    element_ptr: Option<usize>,
) -> BackgroundTargetFacts {
    let window_server = match resolve_window_owner(pid, window_id) {
        WindowOwner::SamePid => WindowServerOwnership::SamePid,
        WindowOwner::Unknown => WindowServerOwnership::NotFound,
        WindowOwner::ForeignPid { owner_pid, .. } => {
            WindowServerOwnership::ForeignPid { owner_pid }
        }
    };

    // SAFETY: the application element is created and released here; window
    // elements are released inside ax_window_records; the caller guarantees
    // element_ptr stays retained.
    let (records, app_hidden, element) = unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            (
                Vec::new(),
                None,
                element_ptr.map(|_| ElementAncestry::Unproven),
            )
        } else {
            // Electron/Chromium apps may need per-process-lifetime enablement
            // before their AX windows and subtrees are materialized.
            super::enablement::ensure_chromium_ax_enabled(pid, app);
            let records = ax_window_records(app);
            let app_hidden = copy_bool_attr(app, "AXHidden");
            let element =
                element_ptr.map(|ptr| element_window_ancestry(ptr as AXUIElementRef, window_id));
            CFRelease(app as CFTypeRef);
            (records, app_hidden, element)
        }
    };

    let target = records.iter().find(|record| record.window_id == window_id);
    let competing_keyboard_destinations = count_competing_keyboard_destinations(
        pid,
        window_id,
        all_windows()
            .iter()
            .map(|window| (window.pid, window.window_id)),
        &records,
    );

    BackgroundTargetFacts {
        window_server,
        ax_window_present: target.is_some(),
        target_minimized: target.and_then(|record| record.minimized),
        app_hidden,
        competing_keyboard_destinations,
        element: element.unwrap_or(ElementAncestry::NotAddressed),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        count_competing_keyboard_destinations, walk_window_ancestry, AncestryElement,
        AxWindowRecord, ElementAncestry, MAX_ANCESTRY_DEPTH,
    };
    use std::cell::Cell;

    #[derive(Clone, Copy)]
    struct Node {
        role: Option<&'static str>,
        id: Option<u32>,
        window: Option<usize>,
        parent: Option<usize>,
    }

    impl Node {
        fn new(role: &'static str, id: Option<u32>, parent: Option<usize>) -> Self {
            Self {
                role: Some(role),
                id,
                window: None,
                parent,
            }
        }
    }

    struct FakeElement<'a> {
        nodes: &'a [Node],
        index: usize,
        reads: &'a Cell<usize>,
    }

    impl FakeElement<'_> {
        fn at(&self, index: usize) -> Self {
            Self {
                nodes: self.nodes,
                index,
                reads: self.reads,
            }
        }
    }

    impl AncestryElement for FakeElement<'_> {
        fn window(&self) -> Option<Self> {
            self.nodes[self.index].window.map(|index| self.at(index))
        }
        fn window_id(&self) -> Option<u32> {
            self.nodes[self.index].id
        }
        fn role(&self) -> Option<String> {
            self.reads.set(self.reads.get() + 1);
            self.nodes[self.index].role.map(str::to_owned)
        }
        fn parent(&self) -> Option<Self> {
            self.nodes[self.index].parent.map(|index| self.at(index))
        }
    }

    fn ancestry(nodes: &[Node], target: u32) -> (ElementAncestry, usize) {
        let reads = Cell::new(0);
        let result = walk_window_ancestry(
            FakeElement {
                nodes,
                index: 0,
                reads: &reads,
            },
            target,
        );
        (result, reads.get())
    }

    #[test]
    fn direct_window_match_keeps_the_fast_path() {
        let mut leaf = Node::new("AXButton", None, None);
        leaf.window = Some(1);
        assert_eq!(
            ancestry(&[leaf, Node::new("AXWindow", Some(10), None)], 10),
            (ElementAncestry::ProvenDescendant, 0)
        );
    }

    #[test]
    fn sheet_identity_does_not_hide_the_requested_parent() {
        let mut leaf = Node::new("AXButton", None, Some(1));
        leaf.window = Some(1);
        let nodes = [
            leaf,
            Node::new("AXSheet", Some(20), Some(2)),
            Node::new("AXWindow", Some(10), Some(3)),
            Node::new("AXApplication", None, None),
        ];
        assert_eq!(ancestry(&nodes, 10), (ElementAncestry::ProvenDescendant, 3));
    }

    #[test]
    fn same_process_sibling_does_not_prove_the_target() {
        let mut leaf = Node::new("AXButton", None, Some(1));
        leaf.window = Some(1);
        let nodes = [
            leaf,
            Node::new("AXWindow", Some(20), Some(2)),
            Node::new("AXApplication", None, None),
        ];
        assert_eq!(ancestry(&nodes, 10).0, ElementAncestry::OutsideTargetWindow);
    }

    #[test]
    fn unmappable_sheet_continues_to_a_mapped_parent() {
        let nodes = [
            Node::new("AXSheet", None, Some(1)),
            Node::new("AXWindow", Some(10), None),
        ];
        assert_eq!(ancestry(&nodes, 10).0, ElementAncestry::ProvenDescendant);
    }

    #[test]
    fn unmapped_or_broken_ancestry_remains_unproven() {
        assert_eq!(
            ancestry(&[Node::new("AXWindow", None, None)], 10).0,
            ElementAncestry::Unproven
        );
        let missing_role = Node {
            role: None,
            ..Node::new("AXGroup", None, Some(1))
        };
        let application = Node::new("AXApplication", None, Some(1));
        for stop in [missing_role, application] {
            assert_eq!(
                ancestry(&[stop, Node::new("AXWindow", Some(10), None)], 10),
                (ElementAncestry::Unproven, 1)
            );
        }
    }

    #[test]
    fn parent_cycles_are_bounded_and_never_prove_a_missing_target() {
        let nodes = [
            Node::new("AXGroup", None, Some(1)),
            Node::new("AXSheet", Some(20), Some(0)),
        ];
        assert_eq!(
            ancestry(&nodes, 10),
            (ElementAncestry::OutsideTargetWindow, MAX_ANCESTRY_DEPTH)
        );
        assert_eq!(
            ancestry(&[Node::new("AXGroup", None, Some(0))], 10),
            (ElementAncestry::Unproven, MAX_ANCESTRY_DEPTH)
        );
    }

    #[test]
    fn target_must_be_inside_the_depth_limit() {
        let mut nodes: Vec<_> = (0..MAX_ANCESTRY_DEPTH)
            .map(|index| Node::new("AXGroup", None, Some(index + 1)))
            .collect();
        nodes.push(Node::new("AXWindow", Some(10), None));
        assert_eq!(
            ancestry(&nodes, 10),
            (ElementAncestry::Unproven, MAX_ANCESTRY_DEPTH)
        );
        nodes[MAX_ANCESTRY_DEPTH - 1] = Node::new("AXWindow", Some(10), None);
        assert_eq!(
            ancestry(&nodes, 10),
            (ElementAncestry::ProvenDescendant, MAX_ANCESTRY_DEPTH)
        );
    }

    fn ax_window(window_id: u32, minimized: Option<bool>) -> AxWindowRecord {
        AxWindowRecord {
            window_id,
            minimized,
        }
    }

    #[test]
    fn compositor_surfaces_do_not_create_keyboard_ambiguity() {
        let rows = [(42, 10), (42, 11), (42, 12), (42, 13), (42, 14), (42, 15)];
        let records = [ax_window(10, Some(false))];

        assert_eq!(
            count_competing_keyboard_destinations(42, 10, rows, &records),
            0
        );
    }

    #[test]
    fn independently_mapped_sibling_remains_ambiguous() {
        let rows = [(42, 10), (42, 11)];
        let records = [ax_window(10, Some(false)), ax_window(11, Some(false))];

        assert_eq!(
            count_competing_keyboard_destinations(42, 10, rows, &records),
            1
        );
    }

    #[test]
    fn minimized_mapped_sibling_is_not_a_keyboard_destination() {
        let rows = [(42, 10), (42, 11)];
        let records = [ax_window(10, Some(false)), ax_window(11, Some(true))];

        assert_eq!(
            count_competing_keyboard_destinations(42, 10, rows, &records),
            0
        );
    }

    #[test]
    fn unmapped_window_server_sibling_is_not_a_keyboard_destination() {
        let rows = [(42, 10), (42, 99), (7, 11)];
        let records = [ax_window(10, Some(false)), ax_window(11, Some(false))];

        assert_eq!(
            count_competing_keyboard_destinations(42, 10, rows, &records),
            0
        );
    }
}
