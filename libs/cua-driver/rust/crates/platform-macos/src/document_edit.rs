//! Keep a document's edited state in step with accessibility text writes.
//!
//! An `AXValue` write, or an `AXSelectedText` insert, changes an `NSTextView`'s
//! text without going through its edit model: no undo entry is registered and
//! the `NSDocument` is not marked edited. TextEdit then never autosaves the
//! change, and closing or saving keeps the old content on disk (Bench v2
//! DS-07). Key events do go through the edit model.
//!
//! After an AX write that changed a document's text, this checks the document
//! window's edited mark (`AXEdited` on its close button). When the mark is
//! still clear, it sends one net-zero edit as key events (a space and a
//! backspace at the end of the text), which marks the document edited without
//! changing its content, and reads the mark again.

use std::time::{Duration, Instant};

use crate::ax::bindings::{
    copy_bool_attr, copy_element_attr, copy_string_attr, AXUIElementRef,
    AXUIElementSetAttributeValue,
};

/// What the document's edited mark showed after an AX text write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentEdited {
    /// The element is not in a document window (no `AXEdited` on its close
    /// button), so there is nothing to save.
    NotADocument,
    /// The document was already marked edited.
    AlreadyEdited,
    /// The driver marked it edited with a net-zero key edit.
    Marked,
    /// It is still not marked edited. A save may keep the old content.
    NotMarked,
}

impl DocumentEdited {
    /// A sentence for the tool result, or nothing when there is nothing to say.
    pub fn note(self) -> &'static str {
        match self {
            DocumentEdited::NotADocument | DocumentEdited::AlreadyEdited => "",
            DocumentEdited::Marked => {
                " The document is marked edited, so it autosaves and a save keeps the new text."
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
            DocumentEdited::AlreadyEdited | DocumentEdited::Marked => Some(true),
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

/// The document window's edited mark, `None` when it has none.
///
/// # Safety
/// `element` must be a live `AXUIElementRef`.
unsafe fn edited_mark(element: AXUIElementRef) -> Option<bool> {
    let window = copy_element_attr(element, "AXWindow")?;
    let close = copy_element_attr(window, "AXCloseButton");
    core_foundation::base::CFRelease(window as _);
    let close = close?;
    let edited = copy_bool_attr(close, "AXEdited");
    core_foundation::base::CFRelease(close as _);
    edited
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
    match edited_mark(element) {
        None => return DocumentEdited::NotADocument,
        Some(true) => return DocumentEdited::AlreadyEdited,
        Some(false) => {}
    }
    if !keys_reach_element_window(pid, element) {
        return DocumentEdited::NotMarked;
    }
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
    let deadline = Instant::now() + Duration::from_millis(600);
    loop {
        if edited_mark(element) == Some(true) {
            // The two keys must cancel out; if the text moved, say so.
            return if copy_string_attr(element, "AXValue") == before {
                DocumentEdited::Marked
            } else {
                DocumentEdited::NotMarked
            };
        }
        if Instant::now() >= deadline {
            return DocumentEdited::NotMarked;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
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
        assert_eq!(DocumentEdited::AlreadyEdited.note(), "");
        assert!(DocumentEdited::Marked.note().contains("marked edited"));
        assert!(DocumentEdited::NotMarked
            .note()
            .contains("may keep the old content"));
        assert_eq!(DocumentEdited::NotADocument.structured(), None);
        assert_eq!(DocumentEdited::Marked.structured(), Some(true));
        assert_eq!(DocumentEdited::NotMarked.structured(), Some(false));
    }
}
