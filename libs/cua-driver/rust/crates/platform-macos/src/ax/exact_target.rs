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
use crate::windows::{
    all_windows_with_space_snapshot, all_windows_without_space_metadata, resolve_window_owner,
    resolve_window_owner_in, window_space_facts_in, WindowEnumeration, WindowInfo, WindowOwner,
};

/// Bounded `AXParent` ascent used when an element does not expose `AXWindow`.
const MAX_ANCESTRY_DEPTH: usize = 40;

/// Resolve the CGWindowID of the top-level AX window that owns `element`.
///
/// Prefers the element's `AXWindow` attribute and falls back to a bounded
/// `AXParent` walk. `None` means ancestry could not be proven — callers must
/// treat that as "not the requested window", never as a wildcard.
///
/// # Safety
///
/// `element` must be a valid `AXUIElementRef` for the duration of the call.
pub unsafe fn element_window_id(element: AXUIElementRef) -> Option<u32> {
    if let Some(window) = copy_element_attr(element, "AXWindow") {
        let window_id = ax_get_window_id(window);
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
                resolved = ax_get_window_id(current);
                break;
            }
            Some("AXApplication") | None => break,
            _ => {}
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
            None => return None,
        }
    }
    if owned {
        CFRelease(current as CFTypeRef);
    }
    resolved
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

/// Convert WindowServer's positive membership fact into the negative fact
/// consumed by the background-input policy. Preserve `None`: unknown Space
/// membership must never be treated as proof that a window is off-Space.
fn off_active_space_from(on_current_space: Option<bool>) -> Option<bool> {
    on_current_space.map(|is_on_current_space| !is_on_current_space)
}

/// Choose the cheap layer-0 census when AX already proves the target and the
/// Space-enriched census only when AX is unresolved. Injected collectors keep
/// the hot/cold path choice testable without a live WindowServer.
fn background_window_census(
    ax_window_present: bool,
    pid: i32,
    window_id: u32,
    without_space_metadata: impl FnOnce() -> Vec<WindowInfo>,
    with_space_metadata: impl FnOnce() -> WindowEnumeration,
) -> (Vec<WindowInfo>, Option<bool>) {
    if ax_window_present {
        return (without_space_metadata(), None);
    }

    let snapshot = with_space_metadata();
    let on_current_space = window_space_facts_in(&snapshot.windows, pid, window_id)
        .and_then(|window| window.on_current_space);
    (snapshot.windows, off_active_space_from(on_current_space))
}

fn window_server_ownership(owner: WindowOwner) -> WindowServerOwnership {
    match owner {
        WindowOwner::SamePid => WindowServerOwnership::SamePid,
        WindowOwner::ForeignPid { owner_pid, .. } => {
            WindowServerOwnership::ForeignPid { owner_pid }
        }
        WindowOwner::Unknown => WindowServerOwnership::NotFound,
    }
}

/// Resolve ownership from the same latest layer-0 census used for Space facts.
/// If that census omits the id, run a fresh any-layer lookup: a still-live
/// accessory window stays addressable, while a close/re-parent race refreshes
/// to NotFound/ForeignPid rather than reusing stale pre-AX evidence.
fn refresh_window_server_ownership(
    windows: &[WindowInfo],
    pid: i32,
    window_id: u32,
    any_layer_lookup: impl FnOnce() -> WindowOwner,
) -> WindowServerOwnership {
    match resolve_window_owner_in(windows, pid, window_id) {
        WindowOwner::Unknown => window_server_ownership(any_layer_lookup()),
        owner => window_server_ownership(owner),
    }
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
            let element = element_ptr.map(|ptr| match element_window_id(ptr as AXUIElementRef) {
                Some(id) if id == window_id => ElementAncestry::ProvenDescendant,
                Some(_) => ElementAncestry::OutsideTargetWindow,
                None => ElementAncestry::Unproven,
            });
            CFRelease(app as CFTypeRef);
            (records, app_hidden, element)
        }
    };

    let target = records.iter().find(|record| record.window_id == window_id);
    // Issue #3458: only the AX-unresolved case consumes the off-active-Space
    // fact, so the happy path (AX resolved) keeps the cheap enumeration and
    // never pays the per-window space query. `on_current_space` stays `None`
    // when the space query cannot answer, and an unknown fact never changes
    // a refusal code.
    let (window_census, off_active_space) = background_window_census(
        target.is_some(),
        pid,
        window_id,
        all_windows_without_space_metadata,
        all_windows_with_space_snapshot,
    );
    let window_server = refresh_window_server_ownership(&window_census, pid, window_id, || {
        resolve_window_owner(pid, window_id)
    });
    let competing_keyboard_destinations = count_competing_keyboard_destinations(
        pid,
        window_id,
        window_census
            .iter()
            .map(|window| (window.pid, window.window_id)),
        &records,
    );

    BackgroundTargetFacts {
        window_server,
        ax_window_present: target.is_some(),
        off_active_space,
        target_minimized: target.and_then(|record| record.minimized),
        app_hidden,
        competing_keyboard_destinations,
        element: element.unwrap_or(ElementAncestry::NotAddressed),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        background_window_census, count_competing_keyboard_destinations, off_active_space_from,
        refresh_window_server_ownership, AxWindowRecord,
    };
    use crate::windows::{WindowBounds, WindowEnumeration, WindowInfo, WindowOwner};
    use cua_driver_core::background_input::{
        decide_background_input, BackgroundAction, BackgroundTargetFacts, ElementAncestry,
        ExactWindowTarget, WindowServerOwnership,
    };
    use std::cell::Cell;

    fn ax_window(window_id: u32, minimized: Option<bool>) -> AxWindowRecord {
        AxWindowRecord {
            window_id,
            minimized,
        }
    }

    fn window(window_id: u32, pid: i32, on_current_space: Option<bool>) -> WindowInfo {
        WindowInfo {
            window_id,
            pid,
            app_name: String::new(),
            title: String::new(),
            bounds: WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            layer: 0,
            z_index: 1,
            is_on_screen: true,
            current_space_id: None,
            on_current_space,
            space_ids: None,
        }
    }

    #[test]
    fn window_space_membership_is_inverted_without_guessing_unknowns() {
        assert_eq!(off_active_space_from(Some(false)), Some(true));
        assert_eq!(off_active_space_from(Some(true)), Some(false));
        assert_eq!(off_active_space_from(None), None);
    }

    #[test]
    fn resolved_ax_target_uses_only_the_metadata_free_census() {
        let cheap_calls = Cell::new(0);
        let enriched_calls = Cell::new(0);
        let (windows, off_active_space) = background_window_census(
            true,
            800,
            42,
            || {
                cheap_calls.set(cheap_calls.get() + 1);
                vec![window(42, 800, None)]
            },
            || {
                enriched_calls.set(enriched_calls.get() + 1);
                WindowEnumeration {
                    windows: vec![],
                    current_space_id: None,
                }
            },
        );

        assert_eq!(cheap_calls.get(), 1);
        assert_eq!(enriched_calls.get(), 0);
        assert_eq!(windows.len(), 1);
        assert_eq!(off_active_space, None);
    }

    #[test]
    fn foreign_same_id_row_cannot_supply_an_off_space_fact() {
        let (windows, off_active_space) =
            background_window_census(false, 800, 42, Vec::new, || WindowEnumeration {
                windows: vec![window(42, 900, Some(false))],
                current_space_id: Some(7),
            });

        assert_eq!(off_active_space, None);
        assert_eq!(
            refresh_window_server_ownership(&windows, 800, 42, || {
                panic!("present layer-0 row must avoid the any-layer fallback")
            }),
            WindowServerOwnership::ForeignPid { owner_pid: 900 }
        );
    }

    #[test]
    fn absent_layer_zero_row_refreshes_from_any_layer_without_reusing_stale_ownership() {
        let same_pid = refresh_window_server_ownership(&[], 800, 42, || WindowOwner::SamePid);
        assert_eq!(same_pid, WindowServerOwnership::SamePid);

        let foreign = refresh_window_server_ownership(&[], 800, 42, || WindowOwner::ForeignPid {
            owner_pid: 900,
            owner_app_name: "Panel Service".into(),
        });
        assert_eq!(
            foreign,
            WindowServerOwnership::ForeignPid { owner_pid: 900 }
        );

        let missing = refresh_window_server_ownership(&[], 800, 42, || WindowOwner::Unknown);
        assert_eq!(missing, WindowServerOwnership::NotFound);

        for ownership in [foreign, missing] {
            let facts = BackgroundTargetFacts {
                window_server: ownership,
                ax_window_present: true,
                off_active_space: None,
                target_minimized: Some(false),
                app_hidden: Some(false),
                competing_keyboard_destinations: 0,
                element: ElementAncestry::NotAddressed,
            };
            assert!(
                !decide_background_input(
                    ExactWindowTarget {
                        pid: 800,
                        window_id: 42,
                    },
                    &facts,
                    BackgroundAction::GenericKey,
                )
                .is_execute(),
                "foreign/missing fallback must refuse process-scoped input"
            );
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
