//! Keep a document's edited state in step with accessibility text writes.
//!
//! An `AXValue` write, or an `AXSelectedText` insert, changes an `NSTextView`'s
//! text without going through its edit model: no undo entry is registered and
//! the `NSDocument` is not marked edited. TextEdit then never autosaves the
//! change, and closing or saving keeps the old content on disk (Bench v2
//! DS-07). Key events do go through the edit model.
//!
//! After an AX write that changed the text of a document window (one with an
//! `AXDocument` URL), this sends one net-zero edit as key events (a space and
//! a backspace), which registers the change with the document without
//! changing its content, and checks that the text did not move. There is no
//! accessibility readback of the edited state: `AXEdited` on the close button
//! stays false for autosave-in-place documents even after a typed edit (VM
//! check, TextEdit, 11 Oct 2026); TextEdit's autosave writing the new text is
//! the evidence.

use std::time::Duration;

use crate::ax::bindings::{
    copy_element_attr, copy_string_attr, AXUIElementRef, AXUIElementSetAttributeValue,
};

/// What the document's edited mark showed after an AX text write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentEdited {
    /// The element is not in a document window, so there is nothing to save.
    NotADocument,
    /// The driver sent a net-zero key edit and the text did not move.
    Marked,
    /// The key edit could not be sent to this window, or the text moved. A
    /// save may keep the old content.
    NotMarked,
}

impl DocumentEdited {
    /// A sentence for the tool result, or nothing when there is nothing to say.
    pub fn note(self) -> &'static str {
        match self {
            DocumentEdited::NotADocument => "",
            DocumentEdited::Marked => {
                " A net-zero key edit (space, backspace) was sent so the document registers the \
                 change and saves it; an accessibility write alone does not."
            }
            DocumentEdited::NotMarked => {
                " Warning: the document is not marked edited, so saving or closing it may keep \
                 the old content. Type the text with key events instead (click the field, then \
                 type_text), or edit it in the foreground."
            }
        }
    }

    /// The structured value, `None` when the element is not in a document.
    pub fn structured(self) -> Option<bool> {
        match self {
            DocumentEdited::NotADocument => None,
            DocumentEdited::NotMarked => Some(false),
            DocumentEdited::Marked => Some(true),
        }
    }
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXValueCreate(the_type: u32, value_ptr: *const std::ffi::c_void) -> *const std::ffi::c_void;
}
const AX_VALUE_CF_RANGE: u32 = 4;

#[repr(C)]
struct CfRange {
    location: isize,
    length: isize,
}

/// Whether `element` sits in a document window (its window has an
/// `AXDocument` URL).
///
/// # Safety
/// `element` must be a live `AXUIElementRef`.
unsafe fn in_document_window(element: AXUIElementRef) -> bool {
    let Some(window) = copy_element_attr(element, "AXWindow") else {
        return false;
    };
    let document = copy_string_attr(window, "AXDocument");
    core_foundation::base::CFRelease(window as _);
    document.is_some_and(|url| !url.is_empty())
}

/// Whether the app's key window (its `AXFocusedWindow`) is the element's
/// window, so pid-routed key events reach this text and not another window.
///
/// # Safety
/// `element` must be a live `AXUIElementRef`.
unsafe fn keys_reach_element_window(pid: i32, element: AXUIElementRef) -> bool {
    let Some(window) = copy_element_attr(element, "AXWindow") else {
        return false;
    };
    let own = crate::ax::bindings::ax_get_window_id(window);
    core_foundation::base::CFRelease(window as _);
    own.is_some() && crate::ax::bindings::focused_window_id_of_pid(pid) == own
}

/// After an AX text write on `element` (in `pid`) that changed its value, make
/// sure its document is marked edited. See the module docs.
///
/// # Safety
/// `element` must be a live `AXUIElementRef` for the duration of the call.
/// `caret_to_end` puts the caret at the end first (after replacing the whole
/// value); otherwise the edit happens at the caret, which an insert leaves
/// collapsed just after the inserted text.
pub unsafe fn ensure_marked_after_ax_write(
    pid: i32,
    element: AXUIElementRef,
    caret_to_end: bool,
) -> DocumentEdited {
    if !in_document_window(element) {
        return DocumentEdited::NotADocument;
    }
    if keys_reach_element_window(pid, element) {
        return send_net_zero_edit(pid, element, caret_to_end);
    }
    // With several windows open, the app's key window is another document,
    // and pid-routed keys go there. Make this window key inside the app
    // through its focus record (no activation, no raise; the user's window
    // keeps key status), send the net-zero edit, and hand key back. The
    // app's AXFocusedWindow does not follow a focus record, so the proof is
    // the document's own modified flag, read through the app's scripting
    // interface (only when Automation is already allowed).
    let Some(own) = element_window_id(element) else {
        return DocumentEdited::NotMarked;
    };
    let Some(path) = document_path(element) else {
        return DocumentEdited::NotMarked;
    };
    let Some(bundle_id) = crate::apps::bundle_id_for_pid(pid) else {
        return DocumentEdited::NotMarked;
    };
    // A document that already has unsaved changes autosaves its whole text,
    // the accessibility write included; no keys are needed.
    match crate::apple_events::document_modified(&bundle_id, &path) {
        Some(true) => return DocumentEdited::Marked,
        // Without the readback the edit cannot be proven; send nothing.
        None => return DocumentEdited::NotMarked,
        Some(false) => {}
    }
    let previous = crate::ax::bindings::focused_window_id_of_pid(pid);
    if !crate::input::skylight::activate_without_raise_with(pid, own, false) {
        return DocumentEdited::NotMarked;
    }
    std::thread::sleep(Duration::from_millis(120));
    let sent = send_net_zero_edit(pid, element, caret_to_end);
    if let Some(previous) = previous {
        crate::input::skylight::activate_without_raise_with(pid, previous, false);
    }
    if sent == DocumentEdited::Marked
        && crate::apple_events::document_modified(&bundle_id, &path) == Some(true)
    {
        DocumentEdited::Marked
    } else {
        DocumentEdited::NotMarked
    }
}

/// The file behind the element's document window.
///
/// # Safety
/// `element` must be a live `AXUIElementRef`.
unsafe fn document_path(element: AXUIElementRef) -> Option<std::path::PathBuf> {
    let window = copy_element_attr(element, "AXWindow")?;
    let url = copy_string_attr(window, "AXDocument");
    core_foundation::base::CFRelease(window as _);
    crate::apple_events::file_url_to_path(&url?)
}

/// Type a space and a backspace into `element` and check the text did not
/// move.
unsafe fn send_net_zero_edit(
    pid: i32,
    element: AXUIElementRef,
    caret_to_end: bool,
) -> DocumentEdited {
    if caret_to_end {
        caret_to_end_of(element);
    }
    let _ = crate::input::ax_actions::focus_element(element as usize);
    let before = copy_string_attr(element, "AXValue");
    if crate::input::keyboard::press_key(pid, "space", &[]).is_err()
        || crate::input::keyboard::press_key(pid, "delete", &[]).is_err()
    {
        return DocumentEdited::NotMarked;
    }
    // Let the two keys land, then check they cancelled out.
    std::thread::sleep(Duration::from_millis(150));
    if copy_string_attr(element, "AXValue") == before {
        DocumentEdited::Marked
    } else {
        DocumentEdited::NotMarked
    }
}

/// The CGWindowID of the window `element` sits in.
///
/// # Safety
/// `element` must be a live `AXUIElementRef`.
unsafe fn element_window_id(element: AXUIElementRef) -> Option<u32> {
    let window = copy_element_attr(element, "AXWindow")?;
    let id = crate::ax::bindings::ax_get_window_id(window);
    core_foundation::base::CFRelease(window as _);
    id
}

/// Collapse the selection to the end of the text.
///
/// # Safety
/// `element` must be a live `AXUIElementRef`.
unsafe fn caret_to_end_of(element: AXUIElementRef) {
    let len = copy_string_attr(element, "AXValue")
        .map(|v| v.encode_utf16().count())
        .unwrap_or(0);
    let range = CfRange {
        location: len as isize,
        length: 0,
    };
    let value = AXValueCreate(AX_VALUE_CF_RANGE, &range as *const CfRange as *const _);
    if !value.is_null() {
        use core_foundation::{base::TCFType, string::CFString};
        let attr = CFString::new("AXSelectedTextRange");
        let _ = AXUIElementSetAttributeValue(element, attr.as_concrete_TypeRef(), value as _);
        core_foundation::base::CFRelease(value as _);
    }
}

#[cfg(test)]
mod tests {
    use super::DocumentEdited;

    #[test]
    fn only_a_document_left_unedited_gets_a_warning() {
        assert_eq!(DocumentEdited::NotADocument.note(), "");
        assert!(DocumentEdited::Marked.note().contains("net-zero key edit"));
        assert!(DocumentEdited::NotMarked
            .note()
            .contains("may keep the old content"));
        assert_eq!(DocumentEdited::NotADocument.structured(), None);
        assert_eq!(DocumentEdited::Marked.structured(), Some(true));
        assert_eq!(DocumentEdited::NotMarked.structured(), Some(false));
    }
}
