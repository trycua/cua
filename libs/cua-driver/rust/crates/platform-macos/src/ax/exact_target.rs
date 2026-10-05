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
    ax_get_window_id, copy_ax_windows_including, copy_bool_attr, copy_element_attr,
    copy_string_attr, focused_element_of_pid, AXUIElementCreateApplication, AXUIElementRef,
};
use super::snapshot::RetainedElement;
use crate::windows::{all_windows, resolve_window_owner, WindowOwner};

/// Bounded `AXParent` ascent used when an element does not expose `AXWindow`.
const MAX_ANCESTRY_DEPTH: usize = 40;

/// Bounded sheet chain followed when mapping a sheet to its parent window.
const MAX_SHEET_NESTING: usize = 8;

/// Resolve the CGWindowID of the top-level AX window that owns `element`.
///
/// Prefers the element's `AXWindow` attribute and falls back to a bounded
/// `AXParent` walk. `None` means ancestry could not be proven — callers must
/// treat that as "not the requested window", never as a wildcard.
///
/// A sheet is its own WindowServer window (and a remote Open/Save panel's
/// content lives in yet another, service-owned one), but it is modal to the
/// window it is attached to and is observed inside that window's snapshot.
/// An element under an `AXSheet` therefore resolves to the sheet's parent
/// window when AX proves that parent (issue #4392).
///
/// # Safety
///
/// `element` must be a valid `AXUIElementRef` for the duration of the call.
pub unsafe fn element_window_id(element: AXUIElementRef) -> Option<u32> {
    if let Some(window) = copy_element_attr(element, "AXWindow") {
        let window_id = owning_window_id(window);
        CFRelease(window as CFTypeRef);
        if window_id.is_some() {
            return window_id;
        }
    }
    // Fallback: ascend AXParent until a window role, then map it.
    let mut current: AXUIElementRef = element;
    let mut owned = false;
    let mut resolved = None;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        match copy_string_attr(current, "AXRole").as_deref() {
            Some("AXWindow") | Some("AXSheet") => {
                resolved = owning_window_id(current);
                break;
            }
            Some("AXApplication") | None => break,
            _ => {}
        }
        let parent = copy_element_attr(current, "AXParent");
        if owned {
            CFRelease(current as CFTypeRef);
        }
        {
            let parent = parent?;
            current = parent;
            owned = true;
        }
    }
    if owned {
        CFRelease(current as CFTypeRef);
    }
    resolved
}

/// Map a window-role AX element to the CGWindowID of the top-level window it
/// belongs to: an `AXWindow` maps to itself, an `AXSheet` to the window it is
/// attached to (see [`element_window_id`]).
///
/// # Safety
///
/// `window` must be a valid `AXUIElementRef` for the duration of the call.
unsafe fn owning_window_id(window: AXUIElementRef) -> Option<u32> {
    resolve_owning_window(
        RetainedElement::retain(window as usize),
        MAX_SHEET_NESTING,
        role_of,
        parent_of,
        |element| ax_get_window_id(element.as_ptr() as AXUIElementRef),
    )
}

fn role_of(element: &RetainedElement) -> Option<String> {
    // SAFETY: a RetainedElement holds a +1 reference on a live element.
    unsafe { copy_string_attr(element.as_ptr() as AXUIElementRef, "AXRole") }
}

fn parent_of(element: &RetainedElement) -> Option<RetainedElement> {
    // SAFETY: as above; copy_element_attr returned +1, handed over here.
    unsafe {
        copy_element_attr(element.as_ptr() as AXUIElementRef, "AXParent").map(|parent| {
            let retained = RetainedElement::retain(parent as usize);
            CFRelease(parent as CFTypeRef);
            retained
        })
    }
}

/// The menu of a menu item whose `AXMenu` parent has no readable `AXParent`.
/// A Mac Catalyst pop-up's menu has this shape: it is its own WindowServer
/// window and names no window upward. Anything else, including an unreadable
/// role, is `None`.
fn parentless_menu<E>(
    item: E,
    role: impl Fn(&E) -> Option<String>,
    parent: impl Fn(&E) -> Option<E>,
) -> Option<E> {
    if role(&item)? != "AXMenuItem" {
        return None;
    }
    let menu = parent(&item)?;
    (role(&menu)? == "AXMenu" && parent(&menu).is_none()).then_some(menu)
}

/// Whether `element` is an item of a menu that names no window (see
/// [`parentless_menu`]).
///
/// # Safety
///
/// `element` must be a valid `AXUIElementRef` for the duration of the call.
pub unsafe fn in_parentless_menu(element: AXUIElementRef) -> bool {
    parentless_menu(
        RetainedElement::retain(element as usize),
        role_of,
        parent_of,
    )
    .is_some()
}

/// Pure sheet-to-parent resolution behind [`owning_window_id`].
///
/// A sheet whose parent is not a window (or a sheet attached to another
/// sheet) is followed up to `max_nesting` levels. When no parent window is
/// proven, the sheet keeps its own window id: that never names a different
/// window than before, so it can only stay refused, never widen.
fn resolve_owning_window<E>(
    start: E,
    max_nesting: usize,
    role: impl Fn(&E) -> Option<String>,
    parent: impl Fn(&E) -> Option<E>,
    window_id: impl Fn(&E) -> Option<u32>,
) -> Option<u32> {
    let own_id = window_id(&start);
    let mut current = start;
    for _ in 0..=max_nesting {
        match role(&current).as_deref() {
            Some("AXSheet") => {}
            Some("AXWindow") => return window_id(&current),
            _ => return own_id,
        }
        current = match parent(&current) {
            Some(next) => next,
            None => return own_id,
        };
    }
    own_id
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
    if element_window_id(element) == Some(window_id) {
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

/// Map the application's fresh `AXWindows` — plus the requested window when it
/// is on another Space — through `_AXUIElementGetWindow`. Windows whose id the
/// SPI cannot resolve are omitted: an unmappable window can never satisfy an
/// exact-target requirement.
unsafe fn ax_window_records(app: AXUIElementRef, pid: i32, window_id: u32) -> Vec<AxWindowRecord> {
    copy_ax_windows_including(app, pid, window_id)
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

/// One same-session WindowServer row as the keyboard-ambiguity count sees it.
struct WindowServerRow {
    pid: i32,
    window_id: u32,
    /// On screen and of real size. An ordered-out window (LibreOffice's
    /// `VCL ImplGetDefaultWindow`, a closed Chrome omnibox popup) cannot be
    /// the key window, so process-scoped keys can never reach it.
    visible: bool,
}

/// Count independently AX-mapped, visible, non-minimized sibling top-level
/// windows that could receive process-scoped key events instead of the target.
///
/// WindowServer may expose several layer-0 compositor surfaces for one native
/// Electron, Tauri, or WebKit window. A raw same-pid CGWindow row is therefore
/// not enough to prove another process-scoped keyboard destination. Requiring a
/// fresh `AXWindows` mapping preserves the fail-closed two-window guard while
/// ignoring render surfaces that cannot independently become the AX key window.
///
/// AppKit sends a process's key events to its key window. When AX proves the
/// target is that window (`target_is_key`), no sibling can receive them, so a
/// sibling such as an open omnibox popup does not make the target ambiguous.
fn count_competing_keyboard_destinations(
    pid: i32,
    target_window_id: u32,
    window_server_rows: impl IntoIterator<Item = WindowServerRow>,
    ax_records: &[AxWindowRecord],
    target_is_key: bool,
) -> usize {
    if target_is_key {
        return 0;
    }
    window_server_rows
        .into_iter()
        .filter(|row| {
            row.pid == pid
                && row.window_id != target_window_id
                && row.visible
                && ax_records.iter().any(|record| {
                    record.window_id == row.window_id && record.minimized != Some(true)
                })
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
    let (records, app_hidden, element, focused_window_id) = unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            (
                Vec::new(),
                None,
                element_ptr.map(|_| ElementAncestry::Unproven),
                None,
            )
        } else {
            // Electron/Chromium apps may need per-process-lifetime enablement
            // before their AX windows and subtrees are materialized.
            super::enablement::ensure_chromium_ax_enabled(pid, app);
            let records = ax_window_records(app, pid, window_id);
            let app_hidden = copy_bool_attr(app, "AXHidden");
            let element = element_ptr.map(|ptr| match element_window_id(ptr as AXUIElementRef) {
                Some(id) if id == window_id => ElementAncestry::ProvenDescendant,
                Some(_) => ElementAncestry::OutsideTargetWindow,
                None => ElementAncestry::Unproven,
            });
            let focused_window_id = copy_element_attr(app, "AXFocusedWindow").and_then(|window| {
                let window_id = ax_get_window_id(window);
                CFRelease(window as CFTypeRef);
                window_id
            });
            CFRelease(app as CFTypeRef);
            (records, app_hidden, element, focused_window_id)
        }
    };

    let target = records.iter().find(|record| record.window_id == window_id);
    let competing_keyboard_destinations = count_competing_keyboard_destinations(
        pid,
        window_id,
        all_windows().iter().map(|window| WindowServerRow {
            pid: window.pid,
            window_id: window.window_id,
            visible: window.is_on_screen
                && window.bounds.width >= 2.0
                && window.bounds.height >= 2.0,
        }),
        &records,
        target.is_some() && focused_window_id == Some(window_id),
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
        count_competing_keyboard_destinations, parentless_menu, resolve_owning_window,
        AxWindowRecord, WindowServerRow,
    };

    /// A fake AX node: (role, AXParent index, CGWindowID).
    type FakeNode = (&'static str, Option<usize>, Option<u32>);

    fn resolve(tree: &[FakeNode], start: usize) -> Option<u32> {
        resolve_owning_window(
            start,
            8,
            |&index| Some(tree[index].0.to_owned()),
            |&index| tree[index].1,
            |&index| tree[index].2,
        )
    }

    #[test]
    fn window_maps_to_itself() {
        assert_eq!(resolve(&[("AXWindow", None, Some(167))], 0), Some(167));
    }

    #[test]
    fn sheet_maps_to_the_window_it_is_attached_to() {
        // Open panel sheet 172 attached to window 167 (issue #4392).
        let tree = [
            ("AXWindow", None, Some(167)),
            ("AXSheet", Some(0), Some(172)),
        ];
        assert_eq!(resolve(&tree, 1), Some(167));
    }

    #[test]
    fn nested_sheets_map_to_the_root_window() {
        let tree = [
            ("AXWindow", None, Some(167)),
            ("AXSheet", Some(0), Some(172)),
            ("AXSheet", Some(1), Some(180)),
        ];
        assert_eq!(resolve(&tree, 2), Some(167));
    }

    #[test]
    fn sheet_without_a_proven_parent_window_keeps_its_own_id() {
        // A top-level consent sheet whose parent is the application, and a
        // sheet whose parent cannot be read, never resolve to another window.
        let tree = [
            ("AXApplication", None, None),
            ("AXSheet", Some(0), Some(172)),
        ];
        assert_eq!(resolve(&tree, 1), Some(172));
        assert_eq!(resolve(&[("AXSheet", None, Some(172))], 0), Some(172));
    }

    #[test]
    fn sheet_chain_is_bounded() {
        let tree = [
            ("AXSheet", Some(1), Some(172)),
            ("AXSheet", Some(0), Some(173)),
        ];
        assert_eq!(resolve(&tree, 0), Some(172));
    }

    /// A fake AX node for the menu checks: (role, AXParent index).
    type MenuNode = (&'static str, Option<usize>);

    fn menu_of(tree: &[MenuNode], item: usize) -> Option<usize> {
        parentless_menu(item, |&i| Some(tree[i].0.to_owned()), |&i| tree[i].1)
    }

    #[test]
    fn only_items_of_a_menu_with_no_parent_qualify() {
        let tree: &[MenuNode] = &[
            // CatalystProfile as measured: the menu has no AXParent.
            ("AXMenu", None),
            ("AXMenuItem", Some(0)),
            // An AppKit pop-up: the menu's parent is the button.
            ("AXPopUpButton", None),
            ("AXMenu", Some(2)),
            ("AXMenuItem", Some(3)),
            // A menu under the application.
            ("AXApplication", None),
            ("AXMenu", Some(5)),
            ("AXMenuItem", Some(6)),
            ("AXButton", Some(0)),
        ];
        assert_eq!(menu_of(tree, 1), Some(0));
        assert_eq!(menu_of(tree, 4), None, "AppKit pop-up");
        assert_eq!(menu_of(tree, 7), None, "a menu under the application");
        assert_eq!(menu_of(tree, 0), None, "the menu itself is not an item");
        assert_eq!(menu_of(tree, 8), None, "not a menu item");
        assert_eq!(
            parentless_menu(1, |_| None, |&i| tree[i].1),
            None,
            "unreadable role"
        );
    }

    fn ax_window(window_id: u32, minimized: Option<bool>) -> AxWindowRecord {
        AxWindowRecord {
            window_id,
            minimized,
        }
    }

    fn rows<const N: usize>(rows: [(i32, u32); N]) -> Vec<WindowServerRow> {
        rows.into_iter()
            .map(|(pid, window_id)| WindowServerRow {
                pid,
                window_id,
                visible: true,
            })
            .collect()
    }

    #[test]
    fn compositor_surfaces_do_not_create_keyboard_ambiguity() {
        let rows = rows([(42, 10), (42, 11), (42, 12), (42, 13), (42, 14), (42, 15)]);
        let records = [ax_window(10, Some(false))];

        assert_eq!(
            count_competing_keyboard_destinations(42, 10, rows, &records, false),
            0
        );
    }

    #[test]
    fn independently_mapped_sibling_remains_ambiguous() {
        let rows = rows([(42, 10), (42, 11)]);
        let records = [ax_window(10, Some(false)), ax_window(11, Some(false))];

        assert_eq!(
            count_competing_keyboard_destinations(42, 10, rows, &records, false),
            1
        );
    }

    #[test]
    fn minimized_mapped_sibling_is_not_a_keyboard_destination() {
        let rows = rows([(42, 10), (42, 11)]);
        let records = [ax_window(10, Some(false)), ax_window(11, Some(true))];

        assert_eq!(
            count_competing_keyboard_destinations(42, 10, rows, &records, false),
            0
        );
    }

    #[test]
    fn unmapped_window_server_sibling_is_not_a_keyboard_destination() {
        let rows = rows([(42, 10), (42, 99), (7, 11)]);
        let records = [ax_window(10, Some(false)), ax_window(11, Some(false))];

        assert_eq!(
            count_competing_keyboard_destinations(42, 10, rows, &records, false),
            0
        );
    }

    #[test]
    fn off_screen_mapped_sibling_is_not_a_keyboard_destination() {
        // LibreOffice lists `VCL ImplGetDefaultWindow` in AXWindows, but it is
        // ordered out and can never be the key window (bench CDB-S02).
        let mut rows = rows([(42, 10), (42, 11)]);
        rows[1].visible = false;
        let records = [ax_window(10, Some(false)), ax_window(11, Some(false))];

        assert_eq!(
            count_competing_keyboard_destinations(42, 10, rows, &records, false),
            0
        );
    }

    #[test]
    fn a_proven_key_target_has_no_competing_destination() {
        // Chrome with its omnibox popup open: the popup is visible and
        // AX-mapped, but the browser window stays key (bench CDB-G04).
        let rows = rows([(42, 10), (42, 11)]);
        let records = [ax_window(10, Some(false)), ax_window(11, Some(false))];

        assert_eq!(
            count_competing_keyboard_destinations(42, 10, rows, &records, true),
            0
        );
    }
}
