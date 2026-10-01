//! Windows cursor-shape tables for the presence hit-test and the real cursor
//! readout: UI Automation control types, `WM_NCHITTEST` codes and `IDC_*`
//! system cursor ids.
//!
//! Pure and platform-neutral (compiled on every host) so the tables are unit
//! tested everywhere; the Win32 and UIA calls that feed them live in
//! [`crate::pointer_shape`] (Windows only).

use cua_driver_core::cursor_shape::{ResizeAxis, SystemCursorShape};

/// UIA control type ids (`UIA_*ControlTypeId`).
pub mod control_type {
    pub const BUTTON: i32 = 50000;
    pub const CHECK_BOX: i32 = 50002;
    pub const COMBO_BOX: i32 = 50003;
    pub const EDIT: i32 = 50004;
    pub const HYPERLINK: i32 = 50005;
    pub const IMAGE: i32 = 50006;
    pub const LIST_ITEM: i32 = 50007;
    pub const MENU_ITEM: i32 = 50011;
    pub const PROGRESS_BAR: i32 = 50012;
    pub const RADIO_BUTTON: i32 = 50013;
    pub const TAB_ITEM: i32 = 50019;
    pub const TEXT: i32 = 50020;
    pub const CUSTOM: i32 = 50025;
    pub const GROUP: i32 = 50026;
    pub const DOCUMENT: i32 = 50030;
    pub const SPLIT_BUTTON: i32 = 50031;
    pub const WINDOW: i32 = 50032;
    pub const PANE: i32 = 50033;
}

/// The attributes of one UIA element the mapping looks at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UiaHitNode {
    /// `CurrentControlType`.
    pub control_type: i32,
    /// `CurrentIsEnabled`; `None` when it could not be read.
    pub enabled: Option<bool>,
    /// `ValuePattern.CurrentIsReadOnly`; `None` when there is no value
    /// pattern.
    pub value_read_only: Option<bool>,
    /// Whether the element supports `TextPattern`.
    pub text_pattern: bool,
}

impl UiaHitNode {
    /// A node with just a control type.
    pub fn of(control_type: i32) -> Self {
        Self {
            control_type,
            ..Self::default()
        }
    }
}

/// Control types that carry no cursor of their own, so the hit-test looks at
/// their parents (a text label inside a button, an image inside a link).
pub fn is_generic_control(control_type: i32) -> bool {
    use control_type::*;
    matches!(control_type, TEXT | IMAGE | GROUP | CUSTOM)
}

/// The shape one element implies on its own, `None` when it implies nothing.
pub fn shape_for_uia(node: &UiaHitNode) -> Option<SystemCursorShape> {
    use control_type::*;
    if node.enabled == Some(false) {
        return Some(SystemCursorShape::Default);
    }
    let editable_value = node.value_read_only == Some(false);
    let shape = match node.control_type {
        EDIT => SystemCursorShape::Text,
        DOCUMENT if node.text_pattern && node.value_read_only != Some(true) => {
            SystemCursorShape::Text
        }
        COMBO_BOX if editable_value => SystemCursorShape::Text,
        HYPERLINK | BUTTON | SPLIT_BUTTON | CHECK_BOX | RADIO_BUTTON | MENU_ITEM | TAB_ITEM
        | COMBO_BOX => SystemCursorShape::Pointer,
        _ => return None,
    };
    Some(shape)
}

/// Resolve the shape at a hit: `chain[0]` is the element under the point,
/// followed by up to three ancestors (nearest first). `in_document` says
/// whether the hit sits inside a `Document` (a browser page), where plain
/// text shows the I-beam. Returns the shape and the index of the deciding
/// element.
pub fn resolve_uia_chain(
    chain: &[UiaHitNode],
    in_document: bool,
) -> (SystemCursorShape, Option<usize>) {
    for (i, node) in chain.iter().take(4).enumerate() {
        if let Some(shape) = shape_for_uia(node) {
            return (shape, Some(i));
        }
        if !is_generic_control(node.control_type) {
            break;
        }
    }
    if in_document
        && chain
            .first()
            .is_some_and(|n| n.control_type == control_type::TEXT)
    {
        return (SystemCursorShape::Text, Some(0));
    }
    (SystemCursorShape::Default, None)
}

/// `WM_NCHITTEST` result codes.
pub mod hit {
    pub const HTLEFT: isize = 10;
    pub const HTRIGHT: isize = 11;
    pub const HTTOP: isize = 12;
    pub const HTTOPLEFT: isize = 13;
    pub const HTTOPRIGHT: isize = 14;
    pub const HTBOTTOM: isize = 15;
    pub const HTBOTTOMLEFT: isize = 16;
    pub const HTBOTTOMRIGHT: isize = 17;
}

/// The resize shape of a `WM_NCHITTEST` code, `None` for anything that is
/// not a sizing border (client area, caption, a non-sizing `HTBORDER`).
pub fn shape_for_nchittest(code: isize) -> Option<SystemCursorShape> {
    use hit::*;
    let axis = match code {
        HTLEFT | HTRIGHT => ResizeAxis::EastWest,
        HTTOP | HTBOTTOM => ResizeAxis::NorthSouth,
        HTTOPLEFT | HTBOTTOMRIGHT => ResizeAxis::NorthWestSouthEast,
        HTTOPRIGHT | HTBOTTOMLEFT => ResizeAxis::NorthEastSouthWest,
        _ => return None,
    };
    Some(SystemCursorShape::Resize(axis))
}

/// Every standard system cursor id (`IDC_*`), in the order the readout
/// loads and compares them.
pub const IDC_IDS: [u16; 16] = [
    32512, 32513, 32514, 32515, 32516, 32642, 32643, 32644, 32645, 32646, 32648, 32649, 32650,
    32651, 32671, 32672,
];

/// The shape of a standard system cursor id, `None` for an unknown id.
pub fn shape_for_idc(id: u16) -> Option<SystemCursorShape> {
    Some(match id {
        32512 => SystemCursorShape::Default,   // IDC_ARROW
        32513 => SystemCursorShape::Text,      // IDC_IBEAM
        32514 => SystemCursorShape::Wait,      // IDC_WAIT
        32515 => SystemCursorShape::Crosshair, // IDC_CROSS
        32516 => SystemCursorShape::Default,   // IDC_UPARROW
        32642 => SystemCursorShape::Resize(ResizeAxis::NorthWestSouthEast), // IDC_SIZENWSE
        32643 => SystemCursorShape::Resize(ResizeAxis::NorthEastSouthWest), // IDC_SIZENESW
        32644 => SystemCursorShape::Resize(ResizeAxis::EastWest), // IDC_SIZEWE
        32645 => SystemCursorShape::Resize(ResizeAxis::NorthSouth), // IDC_SIZENS
        32646 => SystemCursorShape::Resize(ResizeAxis::All), // IDC_SIZEALL
        32648 => SystemCursorShape::NotAllowed, // IDC_NO
        32649 => SystemCursorShape::Pointer,   // IDC_HAND
        32650 => SystemCursorShape::Progress,  // IDC_APPSTARTING
        32651 => SystemCursorShape::Default,   // IDC_HELP
        32671 | 32672 => SystemCursorShape::Pointer, // IDC_PIN, IDC_PERSON
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::control_type::*;
    use super::*;

    #[test]
    fn edits_and_writable_documents_are_ibeams() {
        assert_eq!(
            shape_for_uia(&UiaHitNode::of(EDIT)),
            Some(SystemCursorShape::Text)
        );
        let doc = UiaHitNode {
            text_pattern: true,
            ..UiaHitNode::of(DOCUMENT)
        };
        assert_eq!(shape_for_uia(&doc), Some(SystemCursorShape::Text));
        let read_only = UiaHitNode {
            value_read_only: Some(true),
            ..doc.clone()
        };
        assert_eq!(
            shape_for_uia(&read_only),
            None,
            "a read-only page is not a text box"
        );
        assert_eq!(shape_for_uia(&UiaHitNode::of(DOCUMENT)), None);
    }

    #[test]
    fn clickables_are_hands_and_editable_combos_are_ibeams() {
        for ct in [
            HYPERLINK,
            BUTTON,
            SPLIT_BUTTON,
            CHECK_BOX,
            RADIO_BUTTON,
            MENU_ITEM,
            TAB_ITEM,
            COMBO_BOX,
        ] {
            assert_eq!(
                shape_for_uia(&UiaHitNode::of(ct)),
                Some(SystemCursorShape::Pointer),
                "{ct}"
            );
        }
        let combo = UiaHitNode {
            value_read_only: Some(false),
            ..UiaHitNode::of(COMBO_BOX)
        };
        assert_eq!(shape_for_uia(&combo), Some(SystemCursorShape::Text));
        for ct in [LIST_ITEM, PROGRESS_BAR, PANE, WINDOW] {
            assert_eq!(shape_for_uia(&UiaHitNode::of(ct)), None, "{ct}");
        }
    }

    #[test]
    fn disabled_controls_show_the_arrow() {
        let b = UiaHitNode {
            enabled: Some(false),
            ..UiaHitNode::of(BUTTON)
        };
        assert_eq!(shape_for_uia(&b), Some(SystemCursorShape::Default));
    }

    #[test]
    fn labels_walk_up_to_their_control() {
        let chain = [UiaHitNode::of(TEXT), UiaHitNode::of(BUTTON)];
        assert_eq!(
            resolve_uia_chain(&chain, false),
            (SystemCursorShape::Pointer, Some(1))
        );
        let chain = [
            UiaHitNode::of(IMAGE),
            UiaHitNode::of(GROUP),
            UiaHitNode::of(HYPERLINK),
        ];
        assert_eq!(
            resolve_uia_chain(&chain, true),
            (SystemCursorShape::Pointer, Some(2))
        );
        let chain = [
            UiaHitNode::of(TEXT),
            UiaHitNode::of(PANE),
            UiaHitNode::of(BUTTON),
        ];
        assert_eq!(
            resolve_uia_chain(&chain, false),
            (SystemCursorShape::Default, None)
        );
    }

    #[test]
    fn page_text_is_an_ibeam() {
        let chain = [UiaHitNode::of(TEXT), UiaHitNode::of(GROUP)];
        assert_eq!(
            resolve_uia_chain(&chain, true),
            (SystemCursorShape::Text, Some(0))
        );
        assert_eq!(
            resolve_uia_chain(&chain, false).0,
            SystemCursorShape::Default
        );
    }

    #[test]
    fn sizing_borders_map_to_axes() {
        use hit::*;
        let r = |a| Some(SystemCursorShape::Resize(a));
        assert_eq!(shape_for_nchittest(HTLEFT), r(ResizeAxis::EastWest));
        assert_eq!(shape_for_nchittest(HTRIGHT), r(ResizeAxis::EastWest));
        assert_eq!(shape_for_nchittest(HTTOP), r(ResizeAxis::NorthSouth));
        assert_eq!(shape_for_nchittest(HTBOTTOM), r(ResizeAxis::NorthSouth));
        assert_eq!(
            shape_for_nchittest(HTTOPLEFT),
            r(ResizeAxis::NorthWestSouthEast)
        );
        assert_eq!(
            shape_for_nchittest(HTBOTTOMRIGHT),
            r(ResizeAxis::NorthWestSouthEast)
        );
        assert_eq!(
            shape_for_nchittest(HTTOPRIGHT),
            r(ResizeAxis::NorthEastSouthWest)
        );
        assert_eq!(
            shape_for_nchittest(HTBOTTOMLEFT),
            r(ResizeAxis::NorthEastSouthWest)
        );
        for other in [0, 1, 2, 18, -1] {
            assert_eq!(shape_for_nchittest(other), None, "{other}");
        }
    }

    #[test]
    fn every_standard_cursor_id_has_a_shape() {
        for id in IDC_IDS {
            assert!(shape_for_idc(id).is_some(), "{id}");
        }
        assert_eq!(shape_for_idc(32513), Some(SystemCursorShape::Text));
        assert_eq!(shape_for_idc(32649), Some(SystemCursorShape::Pointer));
        assert_eq!(shape_for_idc(32650), Some(SystemCursorShape::Progress));
        assert_eq!(shape_for_idc(32648), Some(SystemCursorShape::NotAllowed));
        assert_eq!(shape_for_idc(1), None);
    }
}
