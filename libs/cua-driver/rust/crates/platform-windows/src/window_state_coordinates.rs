//! Windows `get_window_state` coordinate-space contract.
//!
//! UIA `BoundingRectangle` values are screen-absolute physical pixels, while
//! pixel actions consume coordinates in the (possibly resized) window PNG.
//! This module keeps the geometry checks and structured-output shape pure so
//! they can be tested without a live Windows desktop.

use serde_json::{json, Value};

pub(crate) const SCREEN_PHYSICAL_PX: &str = "screen_physical_px";
pub(crate) const WINDOW_SCREENSHOT_PX: &str = "window_screenshot_px";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl ScreenRect {
    pub(crate) fn from_edges(left: i32, top: i32, right: i32, bottom: i32) -> Option<Self> {
        let width = right.checked_sub(left)?;
        let height = bottom.checked_sub(top)?;
        (width > 0 && height > 0).then_some(Self {
            x: left,
            y: top,
            width,
            height,
        })
    }

    fn right(self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }

    fn bottom(self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }

    pub(crate) fn intersects(self, other: Self) -> bool {
        i64::from(self.x) < other.right()
            && self.right() > i64::from(other.x)
            && i64::from(self.y) < other.bottom()
            && self.bottom() > i64::from(other.y)
    }
}

/// Add the invariant coordinate spaces and exact target-window bounds to a
/// successful `get_window_state` structured payload.
pub(crate) fn annotate_coordinate_spaces(structured: &mut Value, window_bounds: ScreenRect) {
    structured["window_bounds"] = json!({
        "x": window_bounds.x,
        "y": window_bounds.y,
        "width": window_bounds.width,
        "height": window_bounds.height,
        "coordinate_space": SCREEN_PHYSICAL_PX,
    });
    structured["element_frame_coordinate_space"] = json!(SCREEN_PHYSICAL_PX);
    structured["pixel_action_coordinate_space"] = json!(WINDOW_SCREENSHOT_PX);
}

/// Describe the exact affine mapping the Windows pixel-action path applies:
/// `screen = screen_origin + action_coordinate * screen_pixels_per_action_pixel`.
pub(crate) fn annotate_pixel_action_transform(
    structured: &mut Value,
    screen_origin: (i32, i32),
    screen_pixels_per_action_pixel: (f64, f64),
) {
    structured["pixel_action_to_screen"] = json!({
        "screen_origin": {
            "x": screen_origin.0,
            "y": screen_origin.1,
        },
        "screen_pixels_per_action_pixel": {
            "x": screen_pixels_per_action_pixel.0,
            "y": screen_pixels_per_action_pixel.1,
        },
    });
}

/// Preserve the existing absolute `frame` for a UIA rectangle that intersects
/// the validated target window. A wholly disjoint provider rectangle is not
/// actionable evidence: omit its coordinates and mark why they were rejected.
pub(crate) fn annotate_element_frame(
    entry: &mut Value,
    frame: (i32, i32, i32, i32),
    window_bounds: ScreenRect,
) {
    if let Some(entry) = entry.as_object_mut() {
        entry.remove("frame");
        entry.remove("frame_reliability");
    }
    let Some(frame) = ScreenRect::from_edges(frame.0, frame.1, frame.2, frame.3) else {
        entry["frame_reliability"] = json!("invalid_geometry");
        return;
    };
    if frame.intersects(window_bounds) {
        entry["frame"] = json!({
            "x": frame.x,
            "y": frame.y,
            "w": frame.width,
            "h": frame.height,
        });
    } else {
        entry["frame_reliability"] = json!("outside_target_window");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> ScreenRect {
        ScreenRect {
            x: 692,
            y: 120,
            width: 800,
            height: 600,
        }
    }

    #[test]
    fn rectangle_intersection_accepts_partial_overlap_but_not_touching_edges() {
        assert!(ScreenRect {
            x: 680,
            y: 200,
            width: 20,
            height: 40,
        }
        .intersects(target()));
        assert!(!ScreenRect {
            x: 600,
            y: 200,
            width: 92,
            height: 40,
        }
        .intersects(target()));
    }

    #[test]
    fn valid_element_frame_preserves_backwards_compatible_absolute_shape() {
        let mut entry = json!({ "element_index": 1 });
        annotate_element_frame(&mut entry, (700, 130, 780, 170), target());

        assert_eq!(
            entry["frame"],
            json!({ "x": 700, "y": 130, "w": 80, "h": 40 })
        );
        assert!(entry.get("frame_reliability").is_none());
    }

    #[test]
    fn wholly_disjoint_provider_frame_is_omitted_and_marked_unreliable() {
        let mut entry = json!({
            "element_index": 1,
            "frame": { "x": 6, "y": 6, "w": 80, "h": 40 },
        });
        annotate_element_frame(&mut entry, (6, 6, 86, 46), target());

        assert!(entry.get("frame").is_none());
        assert_eq!(entry["frame_reliability"], "outside_target_window");
    }

    #[test]
    fn coordinate_metadata_is_self_describing_without_another_element_tree() {
        let mut structured = json!({ "elements": [{ "element_index": 1 }] });
        annotate_coordinate_spaces(&mut structured, target());
        annotate_pixel_action_transform(&mut structured, (693, 121), (2.0, 2.0));

        assert_eq!(
            structured,
            json!({
                "elements": [{ "element_index": 1 }],
                "window_bounds": {
                    "x": 692,
                    "y": 120,
                    "width": 800,
                    "height": 600,
                    "coordinate_space": SCREEN_PHYSICAL_PX,
                },
                "element_frame_coordinate_space": SCREEN_PHYSICAL_PX,
                "pixel_action_coordinate_space": WINDOW_SCREENSHOT_PX,
                "pixel_action_to_screen": {
                    "screen_origin": { "x": 693, "y": 121 },
                    "screen_pixels_per_action_pixel": { "x": 2.0, "y": 2.0 },
                },
            })
        );
    }
}
