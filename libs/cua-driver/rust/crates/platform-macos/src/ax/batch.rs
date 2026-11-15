// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Batched attribute reads for the tree walker.
//!
//! Every attribute read is one Mach round trip to the target application,
//! which answers it on its main thread. Reading a node attribute by attribute
//! costs a dozen round trips per node; `AXUIElementCopyMultipleAttributeValues`
//! answers all of them in one. The conversions below mirror the single-read
//! helpers in [`super::bindings`] exactly, so a batched walk renders the
//! same tree as an attribute-by-attribute one. When the application refuses
//! the batched request, every read falls back to those helpers.

use super::bindings::*;
use core_foundation::{
    array::{CFArray, CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef},
    base::{CFGetTypeID, CFRelease, CFRetain, CFTypeRef, TCFType},
    boolean::CFBoolean,
    number::CFNumber,
    string::CFString,
};

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCopyMultipleAttributeValues(
        element: AXUIElementRef,
        attributes: CFArrayRef,
        options: u32,
        values: *mut CFArrayRef,
    ) -> AXError;
    fn AXValueGetTypeID() -> core_foundation::base::CFTypeID;
}

/// The application did not answer within the messaging timeout. Asking again
/// attribute by attribute would only wait out the same timeout once per read.
const AX_ERROR_CANNOT_COMPLETE: AXError = -25204;

/// The attributes the walker reads for a node, in request order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NodeAttr {
    Role,
    Title,
    Value,
    PlaceholderValue,
    Subrole,
    Description,
    Identifier,
    Help,
    Enabled,
    Selected,
    Position,
    Size,
    ValueDescription,
    MinValue,
    MaxValue,
    Children,
}

impl NodeAttr {
    const ALL: [NodeAttr; 16] = [
        NodeAttr::Role,
        NodeAttr::Title,
        NodeAttr::Value,
        NodeAttr::PlaceholderValue,
        NodeAttr::Subrole,
        NodeAttr::Description,
        NodeAttr::Identifier,
        NodeAttr::Help,
        NodeAttr::Enabled,
        NodeAttr::Selected,
        NodeAttr::Position,
        NodeAttr::Size,
        NodeAttr::ValueDescription,
        NodeAttr::MinValue,
        NodeAttr::MaxValue,
        NodeAttr::Children,
    ];

    pub(crate) fn name(self) -> &'static str {
        match self {
            NodeAttr::Role => "AXRole",
            NodeAttr::Title => "AXTitle",
            NodeAttr::Value => "AXValue",
            NodeAttr::PlaceholderValue => "AXPlaceholderValue",
            NodeAttr::Subrole => "AXSubrole",
            NodeAttr::Description => "AXDescription",
            NodeAttr::Identifier => "AXIdentifier",
            NodeAttr::Help => "AXHelp",
            NodeAttr::Enabled => "AXEnabled",
            NodeAttr::Selected => "AXSelected",
            NodeAttr::Position => "AXPosition",
            NodeAttr::Size => "AXSize",
            NodeAttr::ValueDescription => "AXValueDescription",
            NodeAttr::MinValue => "AXMinValue",
            NodeAttr::MaxValue => "AXMaxValue",
            NodeAttr::Children => "AXChildren",
        }
    }

    fn index(self) -> isize {
        NodeAttr::ALL
            .iter()
            .position(|attr| *attr == self)
            .expect("every NodeAttr is listed in ALL") as isize
    }
}

thread_local! {
    static ATTRIBUTE_NAMES: CFArray<CFString> = CFArray::from_CFTypes(
        &NodeAttr::ALL.map(|attr| CFString::new(attr.name())),
    );
}

/// One node's attributes: a single batched reply, or the element itself when
/// the application refused the batch and each read goes to it separately.
pub(crate) enum NodeAttributes {
    Batch(CFArrayRef),
    Single(AXUIElementRef),
    Unanswered,
}

impl Drop for NodeAttributes {
    fn drop(&mut self) {
        if let NodeAttributes::Batch(values) = *self {
            unsafe { CFRelease(values as CFTypeRef) };
        }
    }
}

impl NodeAttributes {
    /// Read every [`NodeAttr`] of `element` in one round trip.
    ///
    /// # Safety
    ///
    /// `element` must be a valid, live `AXUIElementRef` for as long as the
    /// returned value is used.
    pub(crate) unsafe fn read(element: AXUIElementRef) -> Self {
        let mut values: CFArrayRef = std::ptr::null();
        let error = ATTRIBUTE_NAMES.with(|names| {
            AXUIElementCopyMultipleAttributeValues(
                element,
                names.as_concrete_TypeRef(),
                0,
                &mut values,
            )
        });
        if error == kAXErrorSuccess && !values.is_null() {
            if CFArrayGetCount(values) == NodeAttr::ALL.len() as isize {
                return NodeAttributes::Batch(values);
            }
            CFRelease(values as CFTypeRef);
        }
        if error == AX_ERROR_CANNOT_COMPLETE {
            NodeAttributes::Unanswered
        } else {
            NodeAttributes::Single(element)
        }
    }

    /// The batched value of `attr`, borrowed from the reply. Unsupported
    /// attributes come back as an AXError value or kCFNull; the typed readers
    /// below reject both.
    unsafe fn raw(&self, attr: NodeAttr) -> Option<CFTypeRef> {
        match *self {
            NodeAttributes::Batch(values) => {
                let value = CFArrayGetValueAtIndex(values, attr.index()) as CFTypeRef;
                (!value.is_null()).then_some(value)
            }
            _ => None,
        }
    }

    /// [`copy_string_attr`].
    pub(crate) unsafe fn string(&self, attr: NodeAttr) -> Option<String> {
        match *self {
            NodeAttributes::Single(element) => copy_string_attr(element, attr.name()),
            NodeAttributes::Unanswered => None,
            NodeAttributes::Batch(_) => {
                let value = self.raw(attr)?;
                (CFGetTypeID(value) == CFString::type_id())
                    .then(|| CFString::wrap_under_get_rule(value as _).to_string())
            }
        }
    }

    /// [`copy_stringish_attr`].
    pub(crate) unsafe fn stringish(&self, attr: NodeAttr) -> Option<StringishAttrValue> {
        match *self {
            NodeAttributes::Single(element) => copy_stringish_attr(element, attr.name()),
            NodeAttributes::Unanswered => None,
            NodeAttributes::Batch(_) => coerce_stringish_value(self.raw(attr)?),
        }
    }

    /// [`copy_bool_attr`]: a CFBoolean, or a CFNumber read as non-zero.
    pub(crate) unsafe fn bool(&self, attr: NodeAttr) -> Option<bool> {
        match *self {
            NodeAttributes::Single(element) => copy_bool_attr(element, attr.name()),
            NodeAttributes::Unanswered => None,
            NodeAttributes::Batch(_) => {
                let value = self.raw(attr)?;
                let type_id = CFGetTypeID(value);
                if type_id == CFBoolean::type_id() {
                    return Some(CFBoolean::wrap_under_get_rule(value as _).into());
                }
                if type_id == CFNumber::type_id() {
                    return CFNumber::wrap_under_get_rule(value as _)
                        .to_f64()
                        .map(|f| f != 0.0);
                }
                None
            }
        }
    }

    /// [`copy_number_attr`].
    pub(crate) unsafe fn number(&self, attr: NodeAttr) -> Option<f64> {
        match *self {
            NodeAttributes::Single(element) => copy_number_attr(element, attr.name()),
            NodeAttributes::Unanswered => None,
            NodeAttributes::Batch(_) => {
                let value = self.raw(attr)?;
                if CFGetTypeID(value) != CFNumber::type_id() {
                    return None;
                }
                CFNumber::wrap_under_get_rule(value as _).to_f64()
            }
        }
    }

    /// [`element_screen_rect`]: `[x, y, width, height]`, or `None` when either
    /// value is missing or the element is smaller than one point.
    pub(crate) unsafe fn screen_rect(&self) -> Option<[f64; 4]> {
        match *self {
            NodeAttributes::Single(element) => element_screen_rect(element),
            NodeAttributes::Unanswered => None,
            NodeAttributes::Batch(_) => {
                let mut point = [0.0_f64; 2];
                let mut size = [0.0_f64; 2];
                if !self.ax_value(NodeAttr::Position, kAXValueCGPointType, &mut point)
                    || !self.ax_value(NodeAttr::Size, kAXValueCGSizeType, &mut size)
                    || size[0] < 1.0
                    || size[1] < 1.0
                {
                    return None;
                }
                Some([point[0], point[1], size[0], size[1]])
            }
        }
    }

    unsafe fn ax_value(&self, attr: NodeAttr, kind: AXValueType, out: &mut [f64; 2]) -> bool {
        let Some(value) = self.raw(attr) else {
            return false;
        };
        CFGetTypeID(value) == AXValueGetTypeID()
            && AXValueGetValue(value as AXValueRef, kind, out.as_mut_ptr().cast())
    }

    /// Whether the batched read of `attr` came back as an error other than
    /// "no such attribute" or "no value": the application failed to answer it.
    pub(crate) unsafe fn read_failed(&self, attr: NodeAttr) -> bool {
        const AX_VALUE_AX_ERROR_TYPE: AXValueType = 5;
        let Some(value) = self.raw(attr) else {
            return false;
        };
        if CFGetTypeID(value) != AXValueGetTypeID()
            || AXValueGetType(value as AXValueRef) != AX_VALUE_AX_ERROR_TYPE
        {
            return false;
        }
        let mut error: AXError = kAXErrorSuccess;
        AXValueGetValue(
            value as AXValueRef,
            AX_VALUE_AX_ERROR_TYPE,
            (&mut error as *mut AXError).cast(),
        );
        is_failed_read(error)
    }

    /// [`copy_children`]: every child element, retained; release each.
    pub(crate) unsafe fn children(&self) -> Vec<AXUIElementRef> {
        match *self {
            NodeAttributes::Single(element) => copy_children(element),
            NodeAttributes::Unanswered => Vec::new(),
            NodeAttributes::Batch(_) => {
                let Some(value) = self.raw(NodeAttr::Children) else {
                    return Vec::new();
                };
                if CFGetTypeID(value) != CFArray::<CFTypeRef>::type_id() {
                    return Vec::new();
                }
                let array = value as CFArrayRef;
                let ax_type_id = AXUIElementGetTypeID();
                (0..CFArrayGetCount(array))
                    .filter_map(|index| {
                        let item = CFArrayGetValueAtIndex(array, index) as CFTypeRef;
                        (!item.is_null() && CFGetTypeID(item) == ax_type_id).then(|| {
                            CFRetain(item);
                            item as AXUIElementRef
                        })
                    })
                    .collect()
            }
        }
    }
}

/// Whether an AXError in a batched reply means the read failed, as opposed to
/// the element having no such attribute or no value for it.
fn is_failed_read(error: AXError) -> bool {
    error != kAXErrorNoValue && error != kAXErrorAttributeUnsupported
}

#[cfg(test)]
mod tests {
    use super::NodeAttr;

    #[test]
    fn only_an_unanswered_attribute_is_a_failed_read() {
        use super::super::bindings::*;
        assert!(!super::is_failed_read(kAXErrorNoValue));
        assert!(!super::is_failed_read(kAXErrorAttributeUnsupported));
        for error in [
            kAXErrorFailure,
            kAXErrorCannotComplete,
            kAXErrorInvalidUIElement,
            kAXErrorIllegalArgument,
        ] {
            assert!(super::is_failed_read(error), "{error}");
        }
    }

    /// A batched reply as an application sends it for a Calculator-style digit
    /// button: AXTitle unsupported, AXDescription "2", AXIdentifier present but
    /// empty, AXHelp set, a numeric AXValue and a boolean AXEnabled. Every slot
    /// the reply leaves unanswered is an AXError placeholder.
    #[test]
    fn batched_labels_keep_absent_empty_and_typed_values_apart() {
        use super::super::bindings::*;
        use super::NodeAttributes;
        use core_foundation::{
            array::CFArray,
            base::{CFRetain, CFType, CFTypeRef, TCFType},
            boolean::CFBoolean,
            number::CFNumber,
            string::CFString,
        };

        const AX_VALUE_AX_ERROR_TYPE: AXValueType = 5;
        let unsupported = kAXErrorAttributeUnsupported;
        let placeholder = unsafe {
            AXValueCreate(
                AX_VALUE_AX_ERROR_TYPE,
                (&unsupported as *const AXError).cast(),
            )
        };
        assert!(!placeholder.is_null());
        let placeholder = unsafe { CFType::wrap_under_create_rule(placeholder as CFTypeRef) };

        let slots: Vec<CFType> = NodeAttr::ALL
            .iter()
            .map(|attr| match attr {
                NodeAttr::Role => CFString::new("AXButton").as_CFType(),
                NodeAttr::Description => CFString::new("2").as_CFType(),
                NodeAttr::Identifier => CFString::new("").as_CFType(),
                NodeAttr::Help => CFString::new("Digit two").as_CFType(),
                NodeAttr::Value => CFNumber::from(1_i32).as_CFType(),
                NodeAttr::Enabled => CFBoolean::true_value().as_CFType(),
                _ => placeholder.clone(),
            })
            .collect();
        let reply = CFArray::from_CFTypes(&slots);
        // NodeAttributes owns its reply under the create rule and releases it
        // on drop, so hand it a reference of its own.
        unsafe { CFRetain(reply.as_CFTypeRef()) };
        let attrs = NodeAttributes::Batch(reply.as_concrete_TypeRef());

        unsafe {
            assert_eq!(attrs.string(NodeAttr::Role).as_deref(), Some("AXButton"));
            assert_eq!(attrs.string(NodeAttr::Title), None);
            assert_eq!(attrs.string(NodeAttr::Description).as_deref(), Some("2"));
            assert_eq!(attrs.string(NodeAttr::Identifier).as_deref(), Some(""));
            assert_eq!(attrs.string(NodeAttr::Help).as_deref(), Some("Digit two"));
            assert_eq!(attrs.string(NodeAttr::PlaceholderValue), None);
            // A typed value is not a string label, but its state survives.
            assert_eq!(attrs.string(NodeAttr::Value), None);
            let value = attrs.stringish(NodeAttr::Value).expect("numeric AXValue");
            assert_eq!(value.string_value, None);
            assert_eq!(value.state_value, "1");
            assert_eq!(attrs.bool(NodeAttr::Enabled), Some(true));
            assert_eq!(attrs.bool(NodeAttr::Selected), None);
            // An unsupported attribute is absent, not a failed read.
            assert!(!attrs.read_failed(NodeAttr::Title));
        }
        drop(attrs);
        // The reply outlives the drop with exactly the reference we kept.
        assert_eq!(reply.retain_count(), 1);
    }

    #[test]
    fn every_attribute_has_a_distinct_slot_and_name() {
        let mut names: Vec<&str> = NodeAttr::ALL.iter().map(|attr| attr.name()).collect();
        for (slot, attr) in NodeAttr::ALL.iter().enumerate() {
            assert_eq!(attr.index(), slot as isize);
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), NodeAttr::ALL.len());
    }
}
