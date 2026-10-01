//! macOS accessibility role -> cursor shape, for the presence hit-test.
//!
//! Pure and platform-neutral (compiled on every host) so the table is unit
//! tested everywhere; the AX calls that feed it live in
//! [`crate::pointer_shape`] (macOS only).
//!
//! The mapping follows what AppKit and WebKit show over each control:
//! I-beam over editable text (and over web text, which browsers mark
//! selectable), the pointing hand over links and buttons (the product
//! decision for presence: a button reads as clickable), resize over
//! splitters, progress over a running busy indicator, the arrow otherwise.

use cua_driver_core::cursor_shape::{ResizeAxis, SystemCursorShape};

/// The attributes of one AX element the mapping looks at.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AxNode {
    /// `AXRole`, for example "AXButton".
    pub role: String,
    /// `AXSubrole`, for example "AXSearchField".
    pub subrole: String,
    /// `AXEnabled`; `None` when the element does not report it.
    pub enabled: Option<bool>,
    /// Whether `AXValue` is settable (an editable combo box or text area).
    pub editable: bool,
    /// `AXOrientation`, for example "AXVerticalOrientation".
    pub orientation: String,
    /// A busy or progress indicator that is running (indeterminate, or
    /// reporting a value below its maximum).
    pub busy: bool,
}

impl AxNode {
    /// A node with just a role.
    pub fn role(role: &str) -> Self {
        Self {
            role: role.into(),
            ..Self::default()
        }
    }
}

/// Roles that carry no cursor of their own, so the hit-test looks at their
/// parents (a label inside a button, an image inside a link).
pub fn is_generic_role(role: &str) -> bool {
    matches!(
        role,
        "AXGroup" | "AXStaticText" | "AXImage" | "AXUnknown" | "AXLayoutItem" | ""
    )
}

/// The shape one element implies on its own, `None` when it implies nothing
/// (the caller keeps looking up, or settles on the arrow).
pub fn shape_for_ax(node: &AxNode) -> Option<SystemCursorShape> {
    if node.enabled == Some(false) {
        // A disabled control shows the arrow.
        return Some(SystemCursorShape::Default);
    }
    let shape = match node.role.as_str() {
        "AXTextField" | "AXTextArea" | "AXSecureTextField" => SystemCursorShape::Text,
        "AXComboBox" if node.editable => SystemCursorShape::Text,
        _ if node.subrole == "AXSearchField" || node.subrole == "AXSecureTextField" => {
            SystemCursorShape::Text
        }
        "AXLink"
        | "AXButton"
        | "AXPopUpButton"
        | "AXCheckBox"
        | "AXRadioButton"
        | "AXMenuButton"
        | "AXDisclosureTriangle"
        | "AXTab"
        | "AXComboBox" => SystemCursorShape::Pointer,
        "AXBusyIndicator" | "AXProgressIndicator" if node.busy => SystemCursorShape::Progress,
        "AXSplitter" => SystemCursorShape::Resize(
            // A vertical splitter bar separates left from right: resize
            // east-west. A horizontal one separates top from bottom.
            if node.orientation == "AXHorizontalOrientation" {
                ResizeAxis::NorthSouth
            } else {
                ResizeAxis::EastWest
            },
        ),
        _ => return None,
    };
    Some(shape)
}

/// Resolve the shape at a hit: `chain[0]` is the element under the point,
/// followed by up to three of its ancestors (nearest first). `in_web_area`
/// says whether the hit element sits inside an `AXWebArea`.
///
/// Returns the shape and the index into `chain` of the element that decided
/// it (`None` when nothing did and the answer is the arrow).
pub fn resolve_ax_chain(chain: &[AxNode], in_web_area: bool) -> (SystemCursorShape, Option<usize>) {
    for (i, node) in chain.iter().take(4).enumerate() {
        if let Some(shape) = shape_for_ax(node) {
            return (shape, Some(i));
        }
        if !is_generic_role(&node.role) {
            break;
        }
    }
    // Plain web text is selectable, so browsers show the I-beam over it.
    if in_web_area && chain.first().is_some_and(|n| n.role == "AXStaticText") {
        return (SystemCursorShape::Text, Some(0));
    }
    (SystemCursorShape::Default, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(role: &str, f: impl FnOnce(&mut AxNode)) -> AxNode {
        let mut n = AxNode::role(role);
        f(&mut n);
        n
    }

    #[test]
    fn text_roles_are_ibeams() {
        for role in ["AXTextField", "AXTextArea", "AXSecureTextField"] {
            assert_eq!(
                shape_for_ax(&AxNode::role(role)),
                Some(SystemCursorShape::Text),
                "{role}"
            );
        }
        let search = with("AXTextField", |n| n.subrole = "AXSearchField".into());
        assert_eq!(shape_for_ax(&search), Some(SystemCursorShape::Text));
        let combo = with("AXComboBox", |n| n.editable = true);
        assert_eq!(shape_for_ax(&combo), Some(SystemCursorShape::Text));
        assert_eq!(
            shape_for_ax(&AxNode::role("AXComboBox")),
            Some(SystemCursorShape::Pointer),
            "a non-editable combo box is a button"
        );
    }

    #[test]
    fn clickables_are_hands() {
        for role in [
            "AXLink",
            "AXButton",
            "AXPopUpButton",
            "AXCheckBox",
            "AXRadioButton",
            "AXMenuButton",
            "AXDisclosureTriangle",
            "AXTab",
        ] {
            assert_eq!(
                shape_for_ax(&AxNode::role(role)),
                Some(SystemCursorShape::Pointer),
                "{role}"
            );
        }
    }

    #[test]
    fn disabled_controls_show_the_arrow() {
        let b = with("AXButton", |n| n.enabled = Some(false));
        assert_eq!(shape_for_ax(&b), Some(SystemCursorShape::Default));
        let t = with("AXTextField", |n| n.enabled = Some(false));
        assert_eq!(shape_for_ax(&t), Some(SystemCursorShape::Default));
    }

    #[test]
    fn busy_and_splitters() {
        let busy = with("AXBusyIndicator", |n| n.busy = true);
        assert_eq!(shape_for_ax(&busy), Some(SystemCursorShape::Progress));
        assert_eq!(
            shape_for_ax(&AxNode::role("AXProgressIndicator")),
            None,
            "idle progress"
        );
        let v = with("AXSplitter", |n| {
            n.orientation = "AXVerticalOrientation".into()
        });
        assert_eq!(
            shape_for_ax(&v),
            Some(SystemCursorShape::Resize(ResizeAxis::EastWest))
        );
        let h = with("AXSplitter", |n| {
            n.orientation = "AXHorizontalOrientation".into()
        });
        assert_eq!(
            shape_for_ax(&h),
            Some(SystemCursorShape::Resize(ResizeAxis::NorthSouth))
        );
    }

    #[test]
    fn labels_inside_buttons_and_links_walk_up() {
        let chain = [
            AxNode::role("AXStaticText"),
            AxNode::role("AXGroup"),
            AxNode::role("AXLink"),
        ];
        assert_eq!(
            resolve_ax_chain(&chain, true),
            (SystemCursorShape::Pointer, Some(2))
        );
        let chain = [AxNode::role("AXImage"), AxNode::role("AXButton")];
        assert_eq!(
            resolve_ax_chain(&chain, false),
            (SystemCursorShape::Pointer, Some(1))
        );
    }

    #[test]
    fn the_walk_stops_at_a_non_generic_role_and_after_three_parents() {
        let chain = [
            AxNode::role("AXStaticText"),
            AxNode::role("AXCell"),
            AxNode::role("AXButton"),
        ];
        assert_eq!(
            resolve_ax_chain(&chain, false),
            (SystemCursorShape::Default, None)
        );
        let chain = [
            AxNode::role("AXGroup"),
            AxNode::role("AXGroup"),
            AxNode::role("AXGroup"),
            AxNode::role("AXGroup"),
            AxNode::role("AXButton"),
        ];
        assert_eq!(
            resolve_ax_chain(&chain, false).0,
            SystemCursorShape::Default
        );
    }

    #[test]
    fn web_text_is_an_ibeam_but_native_labels_are_not() {
        let chain = [AxNode::role("AXStaticText"), AxNode::role("AXGroup")];
        assert_eq!(
            resolve_ax_chain(&chain, true),
            (SystemCursorShape::Text, Some(0))
        );
        assert_eq!(
            resolve_ax_chain(&chain, false),
            (SystemCursorShape::Default, None)
        );
        assert_eq!(
            resolve_ax_chain(&[], false),
            (SystemCursorShape::Default, None)
        );
    }
}
