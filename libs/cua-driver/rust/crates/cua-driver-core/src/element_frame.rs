//! `get_window_state` element rectangles in the two coordinate spaces a
//! caller uses.
//!
//! `frame` is the element's rectangle in screen coordinates on every platform
//! (the space of `scope:"desktop"` actions). `screenshot_frame` is the same
//! rectangle in pixels of the window screenshot returned by the same response
//! — the space of window-local pointer `x`/`y` — so a caller that grounds on
//! the screenshot can click an element's centre without knowing the window
//! origin or the screenshot's downsizing. Each backend supplies the window
//! origin and the delivered-pixels-per-screen-unit scale of its own capture;
//! the arithmetic is shared here.

use serde_json::{json, Value};

/// Add `screenshot_frame` to every element that has a screen `frame`.
///
/// `origin` is the screen position of the screenshot's top-left pixel and
/// `scale` the number of delivered screenshot pixels per screen unit
/// (`< 1.0` when the capture was downsized, `2.0` for a full-size Retina
/// capture of a point-based frame).
pub fn with_screenshot_frames(elements: Vec<Value>, origin: (f64, f64), scale: f64) -> Vec<Value> {
    if !(scale.is_finite() && scale > 0.0) {
        return elements;
    }
    elements
        .into_iter()
        .map(|mut entry| {
            if let Some(frame) = screenshot_frame(&entry["frame"], origin, scale) {
                entry["screenshot_frame"] = frame;
            }
            entry
        })
        .collect()
}

fn screenshot_frame(frame: &Value, (ox, oy): (f64, f64), scale: f64) -> Option<Value> {
    let x = frame.get("x")?.as_f64()?;
    let y = frame.get("y")?.as_f64()?;
    let w = frame.get("w")?.as_f64()?;
    let h = frame.get("h")?.as_f64()?;
    Some(json!({
        "x": ((x - ox) * scale).round() as i64,
        "y": ((y - oy) * scale).round() as i64,
        "w": (w * scale).round() as i64,
        "h": (h * scale).round() as i64,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_frame_maps_into_the_delivered_screenshot() {
        let elements = vec![
            json!({"element_index": 0, "frame": {"x": 144, "y": 150, "w": 100, "h": 40}}),
            json!({"element_index": 1}),
        ];
        let out = with_screenshot_frames(elements, (44.0, 40.0), 0.5);
        assert_eq!(
            out[0]["screenshot_frame"],
            json!({"x": 50, "y": 55, "w": 50, "h": 20})
        );
        // Unchanged screen frame; no frame, no screenshot_frame.
        assert_eq!(out[0]["frame"]["x"], 144);
        assert!(out[1].get("screenshot_frame").is_none());
    }

    #[test]
    fn point_frames_scale_to_retina_pixels() {
        let elements = vec![json!({"frame": {"x": 110.5, "y": 220.0, "w": 30.0, "h": 10.0}})];
        let out = with_screenshot_frames(elements, (100.0, 200.0), 2.0);
        assert_eq!(
            out[0]["screenshot_frame"],
            json!({"x": 21, "y": 40, "w": 60, "h": 20})
        );
    }

    #[test]
    fn a_degenerate_scale_adds_nothing() {
        let elements = vec![json!({"frame": {"x": 1, "y": 2, "w": 3, "h": 4}})];
        let out = with_screenshot_frames(elements, (0.0, 0.0), 0.0);
        assert!(out[0].get("screenshot_frame").is_none());
    }
}
