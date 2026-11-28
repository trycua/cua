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
    url::CFURL,
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
    Url,
    Children,
}

impl NodeAttr {
    const ALL: [NodeAttr; 17] = [
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
        NodeAttr::Url,
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
            NodeAttr::Url => "AXURL",
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
        match ATTRIBUTE_NAMES.with(|names| copy_multiple(element, names)) {
            Reply::Values(values) => NodeAttributes::Batch(values),
            Reply::Refused => NodeAttributes::Single(element),
            Reply::Unanswered => NodeAttributes::Unanswered,
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
            NodeAttributes::Batch(_) => string_value(self.raw(attr)?),
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

    /// [`copy_url_attr`]: the absolute string of a CFURL.
    pub(crate) unsafe fn url(&self, attr: NodeAttr) -> Option<String> {
        match *self {
            NodeAttributes::Single(element) => copy_url_attr(element),
            NodeAttributes::Unanswered => None,
            NodeAttributes::Batch(_) => url_value(self.raw(attr)?),
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

    /// Every child element, retained; release each. `None` when the
    /// application failed to answer the read, so the element's children are
    /// unknown rather than absent.
    pub(crate) unsafe fn children(&self) -> Option<Vec<AXUIElementRef>> {
        match *self {
            NodeAttributes::Single(element) => match copy_children_checked(element) {
                Ok(children) => Some(children),
                Err(error) if is_failed_read(error) => None,
                Err(_) => Some(Vec::new()),
            },
            NodeAttributes::Unanswered => None,
            NodeAttributes::Batch(_) => {
                if self.read_failed(NodeAttr::Children) {
                    return None;
                }
                let Some(value) = self.raw(NodeAttr::Children) else {
                    return Some(Vec::new());
                };
                if CFGetTypeID(value) != CFArray::<CFTypeRef>::type_id() {
                    return Some(Vec::new());
                }
                let array = value as CFArrayRef;
                let ax_type_id = AXUIElementGetTypeID();
                let children = (0..CFArrayGetCount(array))
                    .filter_map(|index| {
                        let item = CFArrayGetValueAtIndex(array, index) as CFTypeRef;
                        (!item.is_null() && CFGetTypeID(item) == ax_type_id).then(|| {
                            CFRetain(item);
                            item as AXUIElementRef
                        })
                    })
                    .collect();
                Some(children)
            }
        }
    }
}

/// The reply to one `AXUIElementCopyMultipleAttributeValues` request.
enum Reply {
    /// One value per requested name, owned by the caller.
    Values(CFArrayRef),
    /// The application refused the batch; single reads may still answer.
    Refused,
    /// The application did not answer in time; single reads would not either.
    Unanswered,
}

unsafe fn copy_multiple(element: AXUIElementRef, names: &CFArray<CFString>) -> Reply {
    let mut values: CFArrayRef = std::ptr::null();
    let error = AXUIElementCopyMultipleAttributeValues(
        element,
        names.as_concrete_TypeRef(),
        0,
        &mut values,
    );
    if error == kAXErrorSuccess && !values.is_null() {
        if CFArrayGetCount(values) == names.len() {
            return Reply::Values(values);
        }
        CFRelease(values as CFTypeRef);
    }
    if error == AX_ERROR_CANNOT_COMPLETE {
        Reply::Unanswered
    } else {
        Reply::Refused
    }
}

/// [`copy_string_attr`] of one batched slot: a CFString, empty included;
/// anything else (an AXError value, kCFNull, a number) is `None`.
unsafe fn string_value(value: CFTypeRef) -> Option<String> {
    (CFGetTypeID(value) == CFString::type_id())
        .then(|| CFString::wrap_under_get_rule(value as _).to_string())
}

/// [`copy_url_attr`] of one batched slot: a CFURL as its absolute string;
/// anything else (an AXError value, kCFNull, a CFString) is `None`.
unsafe fn url_value(value: CFTypeRef) -> Option<String> {
    (CFGetTypeID(value) == CFURL::type_id()).then(|| {
        CFURL::wrap_under_get_rule(value as _)
            .absolute()
            .get_string()
            .to_string()
    })
}

unsafe fn string_values<const N: usize>(values: CFArrayRef) -> [Option<String>; N] {
    std::array::from_fn(|index| {
        let value = CFArrayGetValueAtIndex(values, index as isize) as CFTypeRef;
        if value.is_null() {
            None
        } else {
            string_value(value)
        }
    })
}

/// [`copy_string_attr`] of each of `names`, in one round trip instead of one
/// per name. An application that refuses the batch is read name by name; one
/// that does not answer in time reads as every attribute missing, as each
/// single read would after waiting out the same timeout.
///
/// # Safety
///
/// `element` must be a valid, live `AXUIElementRef` for the duration of the call.
pub(crate) unsafe fn copy_string_attrs<const N: usize>(
    element: AXUIElementRef,
    names: [&str; N],
) -> [Option<String>; N] {
    let cf_names = CFArray::from_CFTypes(&names.map(CFString::new));
    match copy_multiple(element, &cf_names) {
        Reply::Values(values) => {
            let strings = string_values(values);
            CFRelease(values as CFTypeRef);
            strings
        }
        Reply::Refused => names.map(|name| copy_string_attr(element, name)),
        Reply::Unanswered => std::array::from_fn(|_| None),
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

    #[test]
    fn string_slots_read_like_single_string_reads() {
        use super::super::bindings::*;
        use core_foundation::{
            array::CFArray,
            base::{CFRelease, CFTypeRef, TCFType},
            number::CFNumber,
            string::CFString,
        };
        let label = CFString::new("Submit");
        let empty = CFString::new("");
        let number = CFNumber::from(8);
        let error = kAXErrorAttributeUnsupported;
        let unsupported = unsafe { AXValueCreate(5, (&error as *const AXError).cast()) };
        assert!(!unsupported.is_null());
        let reply = CFArray::from_copyable(&[
            label.as_CFTypeRef(),
            empty.as_CFTypeRef(),
            unsupported as CFTypeRef,
            number.as_CFTypeRef(),
        ]);
        let strings: [Option<String>; 4] =
            unsafe { super::string_values(reply.as_concrete_TypeRef()) };
        unsafe { CFRelease(unsupported as CFTypeRef) };
        // A present-but-empty label stays distinct from a missing one, and a
        // non-string value is missing, exactly as `copy_string_attr` reads them.
        assert_eq!(
            strings,
            [Some("Submit".into()), Some(String::new()), None, None]
        );
    }

    #[test]
    fn url_slots_read_like_single_url_reads() {
        use core_foundation::{
            base::{kCFAllocatorDefault, TCFType},
            string::CFString,
            url::{CFURLCreateWithString, CFURL},
        };
        let base = CFURL::from_path("/docs/", true).expect("file URL");
        let link: CFURL = unsafe {
            TCFType::wrap_under_create_rule(CFURLCreateWithString(
                kCFAllocatorDefault,
                CFString::new("guide?page=2").as_concrete_TypeRef(),
                base.as_concrete_TypeRef(),
            ))
        };
        let text = CFString::new("https://example.com/");
        // A relative CFURL resolves against its base, as `copy_url_attr` does,
        // and a string slot is not a URL.
        assert_eq!(
            unsafe { super::url_value(link.as_CFTypeRef()) }.as_deref(),
            Some("file:///docs/guide?page=2")
        );
        assert_eq!(unsafe { super::url_value(text.as_CFTypeRef()) }, None);
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
