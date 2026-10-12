//! Save and restore the general pasteboard around a driver-owned paste.
//!
//! The background setup route navigates Chrome's omnibox with its own
//! "Paste and Go" menu command, which needs the setup URL on the pasteboard.
//! Every item and every type the user had there is saved first and written
//! back afterwards.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardTypeString};
use objc2_foundation::{NSArray, NSData, NSString};

pub struct PasteboardStash {
    items: Vec<Vec<(Retained<NSString>, Retained<NSData>)>>,
}

impl PasteboardStash {
    /// Save the current contents, then put `text` on the pasteboard.
    pub fn replace_with_text(text: &str) -> Option<Self> {
        unsafe {
            let pasteboard = NSPasteboard::generalPasteboard();
            let mut items = Vec::new();
            if let Some(current) = pasteboard.pasteboardItems() {
                for index in 0..current.count() {
                    let item = current.objectAtIndex(index);
                    let mut saved = Vec::new();
                    let kinds = item.types();
                    for kind_index in 0..kinds.count() {
                        let kind = kinds.objectAtIndex(kind_index);
                        if let Some(data) = item.dataForType(&kind) {
                            saved.push((kind, data));
                        }
                    }
                    items.push(saved);
                }
            }
            pasteboard.clearContents();
            if !pasteboard.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) {
                Self { items }.restore();
                return None;
            }
            Some(Self { items })
        }
    }

    /// Write the saved items back.
    pub fn restore(self) {
        unsafe {
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            if self.items.is_empty() {
                return;
            }
            let mut writers = Vec::new();
            for saved in &self.items {
                let item = NSPasteboardItem::new();
                for (kind, data) in saved {
                    item.setData_forType(data, kind);
                }
                writers.push(ProtocolObject::from_retained(item));
            }
            pasteboard.writeObjects(&NSArray::from_vec(writers));
        }
    }
}
