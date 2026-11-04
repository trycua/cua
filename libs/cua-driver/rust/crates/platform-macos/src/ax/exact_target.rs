// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

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
    ax_get_window_id, copy_ax_windows, copy_ax_windows_including, copy_bool_attr, copy_children,
    copy_element_attr, copy_string_attr, focused_element_of_pid, AXUIElementCreateApplication,
    AXUIElementRef,
};
use super::snapshot::RetainedElement;
use super::window_scope::window_owning_sheet;
use crate::windows::{all_windows_without_spaces, resolve_window_owner, WindowOwner};

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

/// Decide an addressed element's ancestry from what was read about it.
fn element_ancestry(window: Option<u32>, in_app_menu_bar: bool, requested: u32) -> ElementAncestry {
    match window {
        Some(id) if id == requested => ElementAncestry::ProvenDescendant,
        Some(_) => ElementAncestry::OutsideTargetWindow,
        None if in_app_menu_bar => ElementAncestry::ProvenDescendant,
        None => ElementAncestry::Unproven,
    }
}

/// Whether `element` sits under the application's own `AXMenuBar`: a bounded
/// `AXParent` ascent that must end at the same menu bar element the
/// application reports.
///
/// # Safety
///
/// `app` and `element` must be valid `AXUIElementRef`s.
unsafe fn element_in_app_menu_bar(app: AXUIElementRef, element: AXUIElementRef) -> bool {
    let Some(menu_bar) = copy_element_attr(app, "AXMenuBar") else {
        return false;
    };
    let mut current = element;
    let mut owned = false;
    let mut found = false;
    for _ in 0..MAX_ANCESTRY_DEPTH {
        if core_foundation::base::CFEqual(current as CFTypeRef, menu_bar as CFTypeRef) != 0 {
            found = true;
            break;
        }
        if matches!(
            copy_string_attr(current, "AXRole").as_deref(),
            Some("AXWindow") | Some("AXSheet") | Some("AXApplication") | None
        ) {
            break;
        }
        let parent = copy_element_attr(current, "AXParent");
        if owned {
            CFRelease(current as CFTypeRef);
        }
        match parent {
            Some(parent) => {
                current = parent;
                owned = true;
            }
            None => {
                owned = false;
                break;
            }
        }
    }
    if owned {
        CFRelease(current as CFTypeRef);
    }
    CFRelease(menu_bar as CFTypeRef);
    found
}

/// Map a window-role AX element to the CGWindowID of the top-level window it
/// belongs to: an `AXWindow` maps to itself, an `AXSheet` to the window it is
/// attached to (see [`element_window_id`]).
///
/// # Safety
///
/// `window` must be a valid `AXUIElementRef` for the duration of the call.
pub(crate) unsafe fn owning_window_id(window: AXUIElementRef) -> Option<u32> {
    resolve_owning_window(
        RetainedElement::retain(window as usize),
        MAX_SHEET_NESTING,
        |element| copy_string_attr(element.as_ptr() as AXUIElementRef, "AXRole"),
        |element| {
            copy_element_attr(element.as_ptr() as AXUIElementRef, "AXParent").map(|parent| {
                // copy_element_attr returned +1; hand that reference over.
                let retained = RetainedElement::retain(parent as usize);
                CFRelease(parent as CFTypeRef);
                retained
            })
        },
        |element| ax_get_window_id(element.as_ptr() as AXUIElementRef),
    )
}

/// The CGWindowIDs of the sheets attached to `window`: its `AXSheet`
/// children, and theirs, up to [`MAX_SHEET_NESTING`] levels.
///
/// # Safety
///
/// `window` must be a valid `AXUIElementRef` for the duration of the call.
pub(crate) unsafe fn attached_sheet_ids(window: AXUIElementRef) -> Vec<u32> {
    let mut ids = Vec::new();
    let mut level = vec![window];
    for depth in 0..MAX_SHEET_NESTING {
        let mut sheets = Vec::new();
        for element in level {
            for child in copy_children(element) {
                if copy_string_attr(child, "AXRole").as_deref() == Some("AXSheet") {
                    ids.extend(ax_get_window_id(child));
                    sheets.push(child);
                } else {
                    CFRelease(child as CFTypeRef);
                }
            }
            if depth > 0 {
                CFRelease(element as CFTypeRef);
            }
        }
        level = sheets;
    }
    for sheet in level {
        CFRelease(sheet as CFTypeRef);
    }
    ids
}

/// Each of the application's `AXWindows` with the sheets attached to it.
///
/// # Safety
///
/// `app` must be a valid application `AXUIElementRef`.
pub(crate) unsafe fn windows_with_sheets(app: AXUIElementRef) -> Vec<(u32, Vec<u32>)> {
    copy_ax_windows(app)
        .into_iter()
        .filter_map(|window| {
            let row = ax_get_window_id(window).map(|id| (id, attached_sheet_ids(window)));
            CFRelease(window as CFTypeRef);
            row
        })
        .collect()
}

/// `pid`'s windows with the sheets attached to them, read fresh.
pub fn windows_with_sheets_of_pid(pid: i32) -> Vec<(u32, Vec<u32>)> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Vec::new();
        }
        let windows = windows_with_sheets(app);
        CFRelease(app as CFTypeRef);
        windows
    }
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

/// The AX window the requested CGWindowID is judged as: itself when AX lists
/// it, else the window it is attached to as a sheet. A sheet's own id never
/// appears in `AXWindows`, and its elements resolve to that window. The sheet
/// rows are read only when the id is not a listed window.
fn ax_target_window(
    requested: u32,
    records: &[AxWindowRecord],
    windows_with_sheets: impl FnOnce() -> Vec<(u32, Vec<u32>)>,
) -> u32 {
    if records.iter().any(|record| record.window_id == requested) {
        return requested;
    }
    window_owning_sheet(requested, &windows_with_sheets()).unwrap_or(requested)
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
/// duration of this call. Blocking: performs up to two Space-free CGWindowList
/// enumerations and bounded AX reads. Call from a blocking context immediately
/// before deciding.
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
    let (records, ax_target, app_hidden, element, focused_window_id) = unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            (
                Vec::new(),
                window_id,
                None,
                element_ptr.map(|_| ElementAncestry::Unproven),
                None,
            )
        } else {
            // Electron/Chromium apps may need per-process-lifetime enablement
            // before their AX windows and subtrees are materialized.
            super::enablement::ensure_chromium_ax_enabled(pid, app);
            let records = ax_window_records(app, pid, window_id);
            // A sheet's own id stands for the window it is attached to: AX
            // lists only that window, and the sheet's elements resolve to it.
            let ax_target = ax_target_window(window_id, &records, || windows_with_sheets(app));
            let app_hidden = copy_bool_attr(app, "AXHidden");
            let element = element_ptr.map(|ptr| {
                let element = ptr as AXUIElementRef;
                let window = element_window_id(element);
                // A menu-bar item has no window ancestor: the menu bar hangs
                // off the application. It still belongs to this app.
                let in_menu_bar = window.is_none() && element_in_app_menu_bar(app, element);
                element_ancestry(window, in_menu_bar, ax_target)
            });
            let focused_window_id = copy_element_attr(app, "AXFocusedWindow").and_then(|window| {
                let window_id = owning_window_id(window);
                CFRelease(window as CFTypeRef);
                window_id
            });
            CFRelease(app as CFTypeRef);
            (records, ax_target, app_hidden, element, focused_window_id)
        }
    };

    let target = records.iter().find(|record| record.window_id == ax_target);
    let competing_keyboard_destinations = count_competing_keyboard_destinations(
        pid,
        ax_target,
        all_windows_without_spaces()
            .iter()
            .map(|window| WindowServerRow {
                pid: window.pid,
                window_id: window.window_id,
                visible: window.is_on_screen
                    && window.bounds.width >= 2.0
                    && window.bounds.height >= 2.0,
            }),
        &records,
        target.is_some() && focused_window_id == Some(ax_target),
    );

    BackgroundTargetFacts {
        window_server,
        ax_window_present: target.is_some(),
        target_minimized: target.and_then(|record| record.minimized),
        app_hidden,
        competing_keyboard_destinations,
        element: element.unwrap_or(ElementAncestry::NotAddressed),
        screen_locked: crate::session::screen_locked(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        count_competing_keyboard_destinations, resolve_owning_window, AxWindowRecord,
        WindowServerRow,
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
    fn menu_bar_item_is_proven_only_through_the_app_menu_bar() {
        use super::element_ancestry;
        use cua_driver_core::background_input::ElementAncestry::*;
        assert_eq!(element_ancestry(None, true, 7), ProvenDescendant);
        assert_eq!(element_ancestry(None, false, 7), Unproven);
        assert_eq!(element_ancestry(Some(7), false, 7), ProvenDescendant);
        assert_eq!(element_ancestry(Some(8), true, 7), OutsideTargetWindow);
    }

    #[test]
    fn a_sheets_own_id_is_judged_as_the_window_it_is_attached_to() {
        use super::{ax_target_window, element_ancestry};
        use cua_driver_core::background_input::ElementAncestry::*;
        let records = [
            AxWindowRecord {
                window_id: 167,
                minimized: Some(false),
            },
            AxWindowRecord {
                window_id: 200,
                minimized: Some(false),
            },
        ];
        let sheets = || vec![(167, vec![15304]), (200, vec![])];
        // Finder's Go to Folder sheet 15304 on window 167.
        let target = ax_target_window(15304, &records, sheets);
        assert_eq!(target, 167);
        assert_eq!(element_ancestry(Some(167), false, target), ProvenDescendant);
        // An element of the other window stays outside the target.
        assert_eq!(
            element_ancestry(Some(200), false, target),
            OutsideTargetWindow
        );
        // A listed window never reads sheets; an unknown id stays itself.
        assert_eq!(
            ax_target_window(200, &records, || panic!("no sheet read")),
            200
        );
        assert_eq!(ax_target_window(999, &records, sheets), 999);
    }

    #[test]
    fn a_focused_sheet_lets_keys_go_to_the_window_it_is_attached_to() {
        use crate::input::skylight::{prime_exact_window_key, KeyFocus, KeyPrime};
        // Window 167 with its Go to Folder sheet 15304 focused.
        let tree = [
            ("AXWindow", None, Some(167)),
            ("AXSheet", Some(0), Some(15304)),
        ];
        let focus = || KeyFocus {
            window: resolve(&tree, 1),
            element_window: resolve(&tree, 1),
        };
        assert_eq!(
            prime_exact_window_key(167, focus, || false, || false),
            Ok(KeyPrime::AlreadyKey)
        );
        // A different focused window is still refused.
        let other = || KeyFocus {
            window: Some(200),
            element_window: Some(200),
        };
        assert!(prime_exact_window_key(167, other, || false, || false).is_err());
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
