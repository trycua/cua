// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Error-preserving read of the AX facts behind an `ax_window_unresolved`
//! snapshot (issue #5386).
//!
//! The scope decision in [`super::window_scope`] needs only a count of
//! readable `AXWindow` candidates, so the walk drops failed reads, wrong-type
//! values and filtered array entries. This probe reads the same attributes
//! again and keeps every native result: the query error, the value's type,
//! raw and AX-element counts, each top-level item's pid, role and window-id
//! conversion, and `AXFocusedWindow`. It is read-only: no input, activation,
//! AX enablement or content capture. It is a separate observation made after
//! the snapshot, so it can differ from it when the app changes in between.

use super::bindings::*;
use core_foundation::array::CFArray;
use core_foundation::base::{CFEqual, CFGetTypeID, CFRelease, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use serde::Serialize;
use std::time::{Duration, Instant};

/// Top-level items whose attributes are read; the rest are only counted.
pub const ITEM_CAP: usize = 32;
/// Overall budget. Reads stop once it is spent and the report says `partial`.
pub const DEADLINE: Duration = Duration::from_millis(1500);
/// Native AX messaging timeout for every element the probe reads.
const READ_TIMEOUT_SECONDS: f32 = 0.25;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
}

/// A native AXError, with its code and its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AxErrorReport {
    pub code: AXError,
    pub name: &'static str,
}

impl AxErrorReport {
    pub fn new(code: AXError) -> Self {
        Self {
            code,
            name: ax_error_name(code),
        }
    }
}

/// What the attribute query itself returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttrRead {
    /// The query's AXError; `None` on success.
    pub ax_error: Option<AxErrorReport>,
    /// `array`, `ax_element`, `string`, `number`, `boolean`, `null`, or
    /// `cf_type_<id>`. `None` when the query failed.
    pub value_type: Option<String>,
    /// Entries in the array value, before any filtering.
    pub raw_count: Option<usize>,
    /// Array entries that are AX elements.
    pub element_count: Option<usize>,
}

impl AttrRead {
    fn failed(code: AXError) -> Self {
        Self {
            ax_error: Some(AxErrorReport::new(code)),
            value_type: None,
            raw_count: None,
            element_count: None,
        }
    }
}

/// One top-level item of the `AXChildren` ∪ `AXWindows` union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemReport {
    /// Which lists named it: `ax_children`, `ax_windows`, or both.
    pub sources: Vec<&'static str>,
    pub pid: Option<i32>,
    pub pid_error: Option<AxErrorReport>,
    /// The item is the application element itself.
    pub is_application: bool,
    pub role: Option<String>,
    pub role_error: Option<AxErrorReport>,
    pub subrole: Option<String>,
    /// `_AXUIElementGetWindow` result. `0` is reported as returned.
    pub window_id: Option<u32>,
    pub window_id_error: Option<AxErrorReport>,
}

impl ItemReport {
    fn is_window_role(&self) -> bool {
        self.role.as_deref() == Some("AXWindow")
    }

    fn maps_to(&self, requested: u32) -> bool {
        matches!(self.role.as_deref(), Some("AXWindow" | "AXSheet"))
            && self.window_id == Some(requested)
    }
}

/// Counts over the union. `union` counts every distinct item; the other two
/// count only the inspected (capped) items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub union: usize,
    pub inspected: usize,
    /// Inspected items whose role reads `AXWindow`: the snapshot's
    /// `ax_window_count`.
    pub ax_window_role: usize,
    /// Inspected `AXWindow`/`AXSheet` items whose window id is the requested
    /// one. Non-zero means the app now exposes the window.
    pub exact_mapped: usize,
}

/// `AXFocusedWindow` of the application element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FocusReport {
    pub read: AttrRead,
    pub role: Option<String>,
    pub role_error: Option<AxErrorReport>,
    pub window_id: Option<u32>,
    pub window_id_error: Option<AxErrorReport>,
    /// `true`/`false` when a window id was read; `null` when focus is unknown.
    pub matches_requested: Option<bool>,
}

impl FocusReport {
    fn unread(read: AttrRead) -> Self {
        Self {
            read,
            role: None,
            role_error: None,
            window_id: None,
            window_id_error: None,
            matches_requested: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Limits {
    pub item_cap: usize,
    pub deadline_ms: u64,
    pub read_timeout_ms: u64,
}

/// The `ax_diagnostics` object of a verbose `ax_window_unresolved` snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeDiagnostics {
    pub requested_window_id: u32,
    /// `None` when the application element could not be created.
    pub ax_children: Option<AttrRead>,
    pub ax_windows: Option<AttrRead>,
    pub items: Vec<ItemReport>,
    pub counts: Counts,
    pub focused_window: Option<FocusReport>,
    /// True when the deadline stopped the probe before every read ran.
    pub partial: bool,
    pub limits: Limits,
}

impl ScopeDiagnostics {
    fn new(requested_window_id: u32) -> Self {
        Self {
            requested_window_id,
            ax_children: None,
            ax_windows: None,
            items: Vec::new(),
            counts: count(&[], 0, requested_window_id),
            focused_window: None,
            partial: false,
            limits: Limits {
                item_cap: ITEM_CAP,
                deadline_ms: DEADLINE.as_millis() as u64,
                read_timeout_ms: (READ_TIMEOUT_SECONDS * 1000.0) as u64,
            },
        }
    }
}

/// Counts for `items` (the inspected prefix) of a union of `union` items.
pub fn count(items: &[ItemReport], union: usize, requested: u32) -> Counts {
    Counts {
        union,
        inspected: items.len(),
        ax_window_role: items.iter().filter(|i| i.is_window_role()).count(),
        exact_mapped: items.iter().filter(|i| i.maps_to(requested)).count(),
    }
}

/// `matches_requested` for a focused window id: unknown stays unknown.
pub fn focus_matches(window_id: Option<u32>, requested: u32) -> Option<bool> {
    window_id.map(|id| id == requested)
}

/// Read the diagnostics for `window_id` under `pid`.
pub fn probe(pid: i32, window_id: u32) -> ScopeDiagnostics {
    let started = Instant::now();
    let mut report = ScopeDiagnostics::new(window_id);
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return report;
        }
        AXUIElementSetMessagingTimeout(app, READ_TIMEOUT_SECONDS);
        let (children_read, children) = copy_list(app, "AXChildren");
        let (windows_read, windows) = copy_list(app, "AXWindows");
        report.ax_children = Some(children_read);
        report.ax_windows = Some(windows_read);

        let mut union: Vec<(AXUIElementRef, Vec<&'static str>)> = Vec::new();
        for (source, list) in [("ax_children", children), ("ax_windows", windows)] {
            for element in list {
                match union
                    .iter_mut()
                    .find(|(e, _)| CFEqual(*e as CFTypeRef, element as CFTypeRef) != 0)
                {
                    Some((_, sources)) => {
                        sources.push(source);
                        CFRelease(element as CFTypeRef);
                    }
                    None => union.push((element, vec![source])),
                }
            }
        }

        for (element, sources) in union.iter().take(ITEM_CAP) {
            if started.elapsed() >= DEADLINE {
                report.partial = true;
                break;
            }
            report.items.push(read_item(app, *element, sources.clone()));
        }
        report.counts = count(&report.items, union.len(), window_id);
        for (element, _) in union {
            CFRelease(element as CFTypeRef);
        }

        if started.elapsed() >= DEADLINE {
            report.partial = true;
        } else {
            report.focused_window = Some(read_focus(app, window_id));
        }
        CFRelease(app as CFTypeRef);
    }
    report
}

unsafe fn copy_value(element: AXUIElementRef, attr: &str) -> (AXError, CFTypeRef) {
    let name = CFString::new(attr);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, name.as_concrete_TypeRef(), &mut value);
    if err != kAXErrorSuccess && !value.is_null() {
        CFRelease(value);
        value = std::ptr::null();
    }
    (err, value)
}

fn type_name(value: CFTypeRef) -> String {
    if value.is_null() {
        return "null".into();
    }
    let id = unsafe { CFGetTypeID(value) };
    if id == CFArray::<CFTypeRef>::type_id() {
        "array".into()
    } else if id == unsafe { AXUIElementGetTypeID() } {
        "ax_element".into()
    } else if id == CFString::type_id() {
        "string".into()
    } else if id == CFNumber::type_id() {
        "number".into()
    } else if id == CFBoolean::type_id() {
        "boolean".into()
    } else {
        format!("cf_type_{id}")
    }
}

/// Copy an array-valued attribute, keeping its native outcome. Returns the
/// retained AX elements the caller must release.
unsafe fn copy_list(element: AXUIElementRef, attr: &str) -> (AttrRead, Vec<AXUIElementRef>) {
    let (err, value) = copy_value(element, attr);
    if err != kAXErrorSuccess {
        return (AttrRead::failed(err), Vec::new());
    }
    let mut read = AttrRead {
        ax_error: None,
        value_type: Some(type_name(value)),
        raw_count: None,
        element_count: None,
    };
    if value.is_null() {
        return (read, Vec::new());
    }
    if CFGetTypeID(value) != CFArray::<CFTypeRef>::type_id() {
        CFRelease(value);
        return (read, Vec::new());
    }
    let array = CFArray::<CFTypeRef>::wrap_under_create_rule(value as _);
    let ax_type = AXUIElementGetTypeID();
    let elements: Vec<AXUIElementRef> = (0..array.len())
        .filter_map(|i| {
            let item = *array.get(i)?;
            (CFGetTypeID(item) == ax_type).then(|| {
                core_foundation::base::CFRetain(item);
                item as AXUIElementRef
            })
        })
        .collect();
    read.raw_count = Some(array.len() as usize);
    read.element_count = Some(elements.len());
    (read, elements)
}

unsafe fn read_string(
    element: AXUIElementRef,
    attr: &str,
) -> (Option<String>, Option<AxErrorReport>) {
    let (err, value) = copy_value(element, attr);
    if err != kAXErrorSuccess {
        return (None, Some(AxErrorReport::new(err)));
    }
    if value.is_null() {
        return (None, None);
    }
    if CFGetTypeID(value) != CFString::type_id() {
        CFRelease(value);
        return (None, None);
    }
    let string = CFString::wrap_under_create_rule(value as CFStringRef);
    (Some(string.to_string()), None)
}

unsafe fn read_window_id(element: AXUIElementRef) -> (Option<u32>, Option<AxErrorReport>) {
    let mut id: u32 = 0;
    let err = _AXUIElementGetWindow(element, &mut id);
    if err == kAXErrorSuccess {
        (Some(id), None)
    } else {
        (None, Some(AxErrorReport::new(err)))
    }
}

unsafe fn read_item(
    app: AXUIElementRef,
    element: AXUIElementRef,
    sources: Vec<&'static str>,
) -> ItemReport {
    AXUIElementSetMessagingTimeout(element, READ_TIMEOUT_SECONDS);
    let mut pid = 0;
    let pid_err = AXUIElementGetPid(element, &mut pid);
    let (role, role_error) = read_string(element, "AXRole");
    let (subrole, _) = read_string(element, "AXSubrole");
    let (window_id, window_id_error) = read_window_id(element);
    ItemReport {
        sources,
        pid: (pid_err == kAXErrorSuccess).then_some(pid),
        pid_error: (pid_err != kAXErrorSuccess).then(|| AxErrorReport::new(pid_err)),
        is_application: CFEqual(element as CFTypeRef, app as CFTypeRef) != 0,
        role,
        role_error,
        subrole,
        window_id,
        window_id_error,
    }
}

unsafe fn read_focus(app: AXUIElementRef, requested: u32) -> FocusReport {
    let (err, value) = copy_value(app, "AXFocusedWindow");
    if err != kAXErrorSuccess {
        return FocusReport::unread(AttrRead::failed(err));
    }
    let read = AttrRead {
        ax_error: None,
        value_type: Some(type_name(value)),
        raw_count: None,
        element_count: None,
    };
    if value.is_null() || CFGetTypeID(value) != AXUIElementGetTypeID() {
        if !value.is_null() {
            CFRelease(value);
        }
        return FocusReport::unread(read);
    }
    let window = value as AXUIElementRef;
    AXUIElementSetMessagingTimeout(window, READ_TIMEOUT_SECONDS);
    let (role, role_error) = read_string(window, "AXRole");
    let (window_id, window_id_error) = read_window_id(window);
    CFRelease(value);
    FocusReport {
        read,
        role,
        role_error,
        window_id,
        window_id_error,
        matches_requested: focus_matches(window_id, requested),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(role: Option<&str>, window_id: Option<u32>) -> ItemReport {
        ItemReport {
            sources: vec!["ax_windows"],
            pid: Some(7),
            pid_error: None,
            is_application: false,
            role: role.map(str::to_owned),
            role_error: role
                .is_none()
                .then(|| AxErrorReport::new(kAXErrorCannotComplete)),
            subrole: None,
            window_id,
            window_id_error: window_id
                .is_none()
                .then(|| AxErrorReport::new(kAXErrorFailure)),
        }
    }

    /// The snapshot's single `ax_window_count` hides these cases; the counts
    /// keep them apart: a window whose role read failed, a window whose id
    /// conversion failed, a different window, and items past the cap.
    #[test]
    fn counts_separate_union_role_and_exact_mapping() {
        let items = [
            item(None, Some(11)),
            item(Some("AXWindow"), None),
            item(Some("AXWindow"), Some(12)),
            item(Some("AXMenuBar"), Some(11)),
        ];
        assert_eq!(
            count(&items, 40, 11),
            Counts {
                union: 40,
                inspected: 4,
                ax_window_role: 2,
                exact_mapped: 0,
            }
        );
        let mapped = [item(Some("AXSheet"), Some(11))];
        assert_eq!(count(&mapped, 1, 11).exact_mapped, 1);
    }

    #[test]
    fn unknown_focus_is_not_a_different_window() {
        assert_eq!(focus_matches(None, 11), None);
        assert_eq!(focus_matches(Some(12), 11), Some(false));
        assert_eq!(focus_matches(Some(11), 11), Some(true));
    }

    #[test]
    fn native_errors_serialize_with_code_and_name() {
        let mut report = ScopeDiagnostics::new(11);
        report.ax_windows = Some(AttrRead::failed(kAXErrorCannotComplete));
        report.ax_children = Some(AttrRead {
            ax_error: None,
            value_type: Some("array".into()),
            raw_count: Some(2),
            element_count: Some(1),
        });
        report.items = vec![item(None, None)];
        report.focused_window = Some(FocusReport::unread(AttrRead::failed(kAXErrorNoValue)));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(
            json["ax_windows"]["ax_error"],
            serde_json::json!({"code": -25204, "name": "kAXErrorCannotComplete"})
        );
        assert_eq!(json["ax_children"]["raw_count"], 2);
        assert_eq!(json["ax_children"]["element_count"], 1);
        assert_eq!(json["items"][0]["role_error"]["code"], -25204);
        assert_eq!(json["items"][0]["window_id_error"]["code"], -25200);
        assert_eq!(json["focused_window"]["read"]["ax_error"]["code"], -25212);
        assert!(json["focused_window"]["matches_requested"].is_null());
        assert_eq!(json["limits"]["item_cap"], ITEM_CAP);
    }
}
