//! Pure mapping tables for the Linux pointer-shape backend
//! ([`cua_driver_core::pointer_shape`]): AT-SPI roles and X cursor names to
//! [`SystemCursorShape`]. No OS calls, so the tables compile and are tested
//! on every host.
//!
//! ## Linux support and limitations
//!
//! | Session | Hit-test | Real cursor | Probe (warp) |
//! |---|---|---|---|
//! | X11 (Xorg, Xvfb, Xtigervnc) | AT-SPI element at point + window-frame edges (`_NET_CLIENT_LIST_STACKING`, `_NET_FRAME_EXTENTS`) | XFixes cursor name | XTest motion |
//! | Wayland, Hyprland | AT-SPI in window coordinates + client frames from `hyprctl clients` | none | none |
//! | Wayland, GNOME / KDE / other | none | none | none |
//!
//! - X11: the real-cursor readout depends on the cursor carrying a name
//!   (libXcursor names theme cursors; a toolkit that uploads a bare bitmap
//!   reports no name, which reads as "unknown", and the caller falls back to
//!   the hit-test). Edges assume every managed, non-maximized normal window is
//!   resizable; fixed-size dialogs still report resize at their border.
//!   Client-side-decorated windows (GTK headerbars) get a 4 px band because
//!   their resize margin lives inside `_GTK_FRAME_EXTENTS`.
//! - Plain push buttons: GTK and Qt draw the arrow over them, but presence
//!   renders the pointing hand for buttons (the product rule: "clickable ->
//!   hand"), so a button hovered by another participant shows the hand even
//!   where the guest would show the arrow.
//! - Wayland: the compositor owns the cursor image and never exposes it to
//!   clients, and clients cannot move the pointer, so neither the real cursor
//!   nor the probe exists. Hyprland exposes window geometry and stacking
//!   (focus history approximates z-order, so a floating window behind the
//!   focused one can be missed) and AT-SPI answers in window coordinates.
//!   GNOME and KDE expose no window geometry to clients, so no hit-test runs
//!   there and every cursor is the arrow.

use cua_driver_core::cursor_shape::{ResizeAxis, SystemCursorShape};

/// One node on the path from a top-level frame down to the deepest
/// accessible under the point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleSample {
    /// AT-SPI role name as `GetRoleName` returns it ("push button").
    pub role: String,
    /// `STATE_EDITABLE` is set.
    pub editable: bool,
    /// `STATE_BUSY` is set.
    pub busy: bool,
}

impl RoleSample {
    pub fn new(role: &str, editable: bool, busy: bool) -> Self {
        Self {
            role: role.to_owned(),
            editable,
            busy,
        }
    }
}

/// How many ancestors above the deepest hit may decide the shape. A label
/// inside a button, or a text run inside an entry, is the deepest hit; the
/// control a level or two up is what the user is pointing at.
pub const DECIDING_ANCESTORS: usize = 3;

/// The shape a single AT-SPI role implies, or `None` when the role implies
/// nothing (the caller keeps looking up the chain, then uses the arrow).
pub fn shape_for_atspi_role(role: &str, editable: bool) -> Option<SystemCursorShape> {
    let role = role.trim().to_ascii_lowercase();
    match role.as_str() {
        "text" | "entry" | "password text" | "terminal" | "document text" | "paragraph"
        | "spin button" | "editbar" | "date editor" => Some(SystemCursorShape::Text),
        "link" | "push button" | "button" | "toggle button" | "check box" | "radio button"
        | "menu item" | "check menu item" | "radio menu item" | "page tab" | "combo box"
        | "push button menu" | "menu" => Some(SystemCursorShape::Pointer),
        _ if editable => Some(SystemCursorShape::Text),
        _ => None,
    }
}

/// Decide the shape for a hit chain ordered frame first, deepest last.
/// Returns the shape and the index of the node that decided it (`None` when
/// nothing did and the arrow applies).
///
/// Busy anywhere on the chain wins (an app busy loading shows progress
/// everywhere in its window); otherwise the deepest node, then up to
/// [`DECIDING_ANCESTORS`] ancestors, may decide.
pub fn shape_for_atspi_chain(chain: &[RoleSample]) -> (SystemCursorShape, Option<usize>) {
    if let Some(i) = chain.iter().rposition(|n| n.busy) {
        return (SystemCursorShape::Progress, Some(i));
    }
    for (i, node) in chain.iter().enumerate().rev().take(DECIDING_ANCESTORS + 1) {
        if let Some(shape) = shape_for_atspi_role(&node.role, node.editable) {
            return (shape, Some(i));
        }
    }
    (SystemCursorShape::Default, None)
}

/// Map an X cursor name (XFixes `GetCursorImageAndName`) onto the portable
/// vocabulary. Covers the core X font names, the freedesktop/CSS names, and
/// the Xcursor hash aliases that common themes ship. An empty name is
/// [`SystemCursorShape::Unknown`] (a bitmap cursor with no name); any other
/// unrecognized name is the arrow.
pub fn shape_for_xcursor_name(name: &str) -> SystemCursorShape {
    use ResizeAxis::*;
    use SystemCursorShape::*;
    let name = name.trim();
    if name.is_empty() {
        return Unknown;
    }
    match name.to_ascii_lowercase().as_str() {
        "left_ptr" | "default" | "arrow" | "top_left_arrow" | "right_ptr" | "context-menu"
        | "help" | "question_arrow" | "whats_this" | "copy" | "alias" | "link" | "dnd-copy"
        | "dnd-link" | "dnd-none" | "center_ptr" => Default,
        "xterm" | "text" | "ibeam" | "cell" => Text,
        "vertical-text" => VerticalText,
        "hand1"
        | "hand2"
        | "pointer"
        | "pointing_hand"
        | "hand"
        | "e29285e634086352946a0e7090d73106"
        | "9d800788f1b08800ae810202380a0822" => Pointer,
        "watch" | "wait" => Wait,
        "left_ptr_watch"
        | "progress"
        | "half-busy"
        | "00000000000000020006000e7e9ffc3f"
        | "08e8e1c95fe2fc01f976f1e063a24ccd"
        | "3ecb610c1bf2410f44200f48c40d3599" => Progress,
        "crossed_circle"
        | "not-allowed"
        | "forbidden"
        | "no-drop"
        | "circle"
        | "03b6e0fcb3499374a867c041f52298f0" => NotAllowed,
        "crosshair" | "cross" | "tcross" | "cross_reverse" | "diamond_cross" => Crosshair,
        "fleur"
        | "move"
        | "all-scroll"
        | "size_all"
        | "dnd-move"
        | "4498f0e0c1937ffe01fd06f973665830"
        | "9081237383d90e509aa00f00170e968f" => Resize(All),
        "grab" | "openhand" | "hand_open" | "5aca4d189052212118709018842178c0" => Grab,
        "grabbing" | "closedhand" | "fist" | "208530c400c041818281048008011002" => Grabbing,
        "sb_h_double_arrow"
        | "h_double_arrow"
        | "ew-resize"
        | "left_side"
        | "right_side"
        | "e-resize"
        | "w-resize"
        | "size_hor"
        | "028006030e0e7ebffc7f7070c0600140"
        | "14fef782d02440884392942c11205230" => Resize(EastWest),
        "col-resize" | "split_h" => Resize(Column),
        "sb_v_double_arrow"
        | "v_double_arrow"
        | "ns-resize"
        | "top_side"
        | "bottom_side"
        | "n-resize"
        | "s-resize"
        | "size_ver"
        | "00008160000006810000408080010102"
        | "2870a09082c103050810ffdffffe0204" => Resize(NorthSouth),
        "row-resize" | "split_v" => Resize(Row),
        "top_left_corner"
        | "bottom_right_corner"
        | "nwse-resize"
        | "size_fdiag"
        | "nw-resize"
        | "se-resize"
        | "c7088f0f3e6c8088236ef8e1e3e70000"
        | "38c5dff7c7b8962045400281044508d2" => Resize(NorthWestSouthEast),
        "top_right_corner"
        | "bottom_left_corner"
        | "nesw-resize"
        | "size_bdiag"
        | "ne-resize"
        | "sw-resize"
        | "fcf1c3c7cd4491d801f1e1c78f100000"
        | "50585d75b494802d0151028115016902" => Resize(NorthEastSouthWest),
        _ => Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::cursor_shape::ResizeAxis::*;
    use SystemCursorShape::*;

    fn s(role: &str) -> RoleSample {
        RoleSample::new(role, false, false)
    }

    #[test]
    fn text_roles_are_ibeam() {
        for role in [
            "text",
            "entry",
            "password text",
            "terminal",
            "document text",
            "Terminal",
        ] {
            assert_eq!(shape_for_atspi_role(role, false), Some(Text), "{role}");
        }
        assert_eq!(
            shape_for_atspi_role("panel", true),
            Some(Text),
            "editable state"
        );
    }

    #[test]
    fn clickable_roles_are_the_hand() {
        for role in [
            "link",
            "push button",
            "toggle button",
            "check box",
            "radio button",
            "menu item",
            "page tab",
            "combo box",
        ] {
            assert_eq!(shape_for_atspi_role(role, false), Some(Pointer), "{role}");
        }
    }

    #[test]
    fn inert_roles_decide_nothing() {
        for role in ["panel", "filler", "label", "frame", "scroll pane", ""] {
            assert_eq!(shape_for_atspi_role(role, false), None, "{role}");
        }
    }

    #[test]
    fn a_label_inside_a_button_is_the_hand() {
        let chain = [s("frame"), s("panel"), s("push button"), s("label")];
        assert_eq!(shape_for_atspi_chain(&chain), (Pointer, Some(2)));
    }

    #[test]
    fn the_deepest_decisive_node_wins() {
        // An entry inside a clickable list row: typing there shows the I-beam.
        let chain = [s("frame"), s("list item"), s("push button"), s("entry")];
        assert_eq!(shape_for_atspi_chain(&chain), (Text, Some(3)));
    }

    #[test]
    fn distant_ancestors_do_not_decide() {
        let chain = [
            s("frame"),
            s("push button"),
            s("panel"),
            s("panel"),
            s("panel"),
            s("panel"),
        ];
        assert_eq!(shape_for_atspi_chain(&chain), (Default, None));
    }

    #[test]
    fn busy_anywhere_is_progress() {
        let chain = [
            RoleSample::new("frame", false, true),
            s("panel"),
            s("entry"),
        ];
        assert_eq!(shape_for_atspi_chain(&chain), (Progress, Some(0)));
        assert_eq!(shape_for_atspi_chain(&[]), (Default, None));
    }

    #[test]
    fn xcursor_names_cover_the_vocabulary() {
        let cases: &[(&str, SystemCursorShape)] = &[
            ("left_ptr", Default),
            ("default", Default),
            ("arrow", Default),
            ("xterm", Text),
            ("text", Text),
            ("ibeam", Text),
            ("hand1", Pointer),
            ("hand2", Pointer),
            ("pointer", Pointer),
            ("pointing_hand", Pointer),
            ("watch", Wait),
            ("wait", Wait),
            ("left_ptr_watch", Progress),
            ("progress", Progress),
            ("half-busy", Progress),
            ("crossed_circle", NotAllowed),
            ("not-allowed", NotAllowed),
            ("forbidden", NotAllowed),
            ("no-drop", NotAllowed),
            ("crosshair", Crosshair),
            ("cross", Crosshair),
            ("tcross", Crosshair),
            ("fleur", Resize(All)),
            ("move", Resize(All)),
            ("all-scroll", Resize(All)),
            ("dnd-move", Resize(All)),
            ("grab", Grab),
            ("openhand", Grab),
            ("grabbing", Grabbing),
            ("closedhand", Grabbing),
            ("sb_h_double_arrow", Resize(EastWest)),
            ("h_double_arrow", Resize(EastWest)),
            ("ew-resize", Resize(EastWest)),
            ("left_side", Resize(EastWest)),
            ("right_side", Resize(EastWest)),
            ("col-resize", Resize(Column)),
            ("sb_v_double_arrow", Resize(NorthSouth)),
            ("v_double_arrow", Resize(NorthSouth)),
            ("ns-resize", Resize(NorthSouth)),
            ("top_side", Resize(NorthSouth)),
            ("bottom_side", Resize(NorthSouth)),
            ("row-resize", Resize(Row)),
            ("top_left_corner", Resize(NorthWestSouthEast)),
            ("bottom_right_corner", Resize(NorthWestSouthEast)),
            ("nwse-resize", Resize(NorthWestSouthEast)),
            ("size_fdiag", Resize(NorthWestSouthEast)),
            ("top_right_corner", Resize(NorthEastSouthWest)),
            ("bottom_left_corner", Resize(NorthEastSouthWest)),
            ("nesw-resize", Resize(NorthEastSouthWest)),
            ("size_bdiag", Resize(NorthEastSouthWest)),
            ("e29285e634086352946a0e7090d73106", Pointer),
            ("08e8e1c95fe2fc01f976f1e063a24ccd", Progress),
            ("03b6e0fcb3499374a867c041f52298f0", NotAllowed),
            ("vertical-text", VerticalText),
        ];
        for (name, want) in cases {
            assert_eq!(&shape_for_xcursor_name(name), want, "{name}");
        }
    }

    #[test]
    fn empty_names_are_unknown_and_strange_names_the_arrow() {
        assert_eq!(shape_for_xcursor_name(""), Unknown);
        assert_eq!(shape_for_xcursor_name("  "), Unknown);
        assert_eq!(shape_for_xcursor_name("my-app-custom-cursor"), Default);
        assert_eq!(shape_for_xcursor_name("XTERM"), Text, "case-insensitive");
    }
}
