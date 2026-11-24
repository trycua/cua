// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Bounds check for window-local pointer coordinates.
//!
//! Pixel clicks take `x`/`y` in the target window's screenshot frame and add
//! the frame's screen origin. A caller that passes screen pixels instead gets
//! a point offset by that origin. On the primary monitor the origin is near
//! `(0, 0)`, so the mistake goes unnoticed; on a secondary monitor it pushes
//! the point past the window, often past the virtual desktop, and the clamped
//! `SendInput` lands on whatever sits there (#3613). Refuse such a point
//! before any input, with the `point_outside_window` code Linux uses.
//!
//! Lives outside the Windows-only modules so its tests run on any host.

use cua_driver_core::protocol::ToolResult;
use serde_json::json;

/// Screen-space frame of a window's screenshot: top-left and size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WindowFrame {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

/// Map window-local `(px, py)` to the screen pixel that contains it, or refuse
/// it when it lies outside `frame`. The check runs on the unrounded point, so a
/// fraction just past an edge is refused rather than rounded or truncated
/// back onto that edge, and pixel `i` covers `[i, i + 1)`.
pub(crate) fn window_point_to_screen(
    hwnd: u64,
    frame: WindowFrame,
    px: f64,
    py: f64,
) -> Result<(i32, i32), ToolResult> {
    let inside =
        px >= 0.0 && py >= 0.0 && px < f64::from(frame.width) && py < f64::from(frame.height);
    // `as` saturates out-of-range and NaN values, which only reach the refusal.
    let screen = (
        frame.left.saturating_add(px.floor() as i32),
        frame.top.saturating_add(py.floor() as i32),
    );
    if inside {
        return Ok(screen);
    }
    let WindowFrame {
        left,
        top,
        width,
        height,
    } = frame;
    let hint = format!(
        "x/y are window-local pixels of window {hwnd} (0..{width} x 0..{height}, its top-left \
         is at screen ({left}, {top})). Pass window-local coordinates from get_window_state's \
         screenshot, or click with scope:\"desktop\" and no pid/window_id for screen pixels \
         (count:2 for a double-click, button:\"right\" for a right-click)."
    );
    Err(ToolResult::error(format!(
        "window-local point ({px}, {py}) of window {hwnd} resolves to screen ({}, {}), outside \
         the window's bounds (x={left}, y={top}, {width}x{height}); no input was sent. {hint}",
        screen.0, screen.1
    ))
    .with_structured(json!({
        "code": "point_outside_window",
        "effect": "refused",
        "target_window": hwnd,
        "screen_point": [screen.0, screen.1],
        "window_point": [px, py],
        "window_bounds": { "x": left, "y": top, "width": width, "height": height },
        "hint": hint,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window from #3613: maximized on a secondary monitor at x=1920, so
    /// its screenshot frame starts at about (1920, 0).
    const SECONDARY: WindowFrame = WindowFrame {
        left: 1920,
        top: 0,
        width: 1920,
        height: 1032,
    };

    #[test]
    fn point_inside_the_window_maps_by_the_frame_origin() {
        assert_eq!(
            window_point_to_screen(0x130cb4, SECONDARY, 1308.0, 770.0).ok(),
            Some((3228, 770))
        );
        assert_eq!(
            window_point_to_screen(0x130cb4, SECONDARY, 0.0, 0.0).ok(),
            Some((1920, 0))
        );
        assert_eq!(
            window_point_to_screen(0x130cb4, SECONDARY, 1919.75, 1031.5).ok(),
            Some((3839, 1031))
        );
    }

    #[test]
    fn screen_pixels_passed_as_window_pixels_are_refused() {
        // The reporter's screen point (3228, 770), sent as window-local x/y.
        let refusal = window_point_to_screen(0x130cb4, SECONDARY, 3228.0, 770.0).unwrap_err();
        let structured = refusal.structured_content.expect("structured refusal");
        assert_eq!(structured["code"], "point_outside_window");
        assert_eq!(structured["effect"], "refused");
        assert_eq!(structured["screen_point"], json!([5148, 770]));
        assert_eq!(structured["window_point"], json!([3228.0, 770.0]));
    }

    #[test]
    fn every_edge_past_the_frame_is_refused() {
        // Fractions just past an edge too: neither truncation nor rounding
        // may pull them back inside.
        for (px, py) in [
            (-1.0, 0.0),
            (0.0, -1.0),
            (1920.0, 0.0),
            (0.0, 1032.0),
            (-0.25, 0.0),
            (0.0, -0.25),
            (1919.0, 1032.25),
            (f64::NAN, 0.0),
        ] {
            let refusal = window_point_to_screen(1, SECONDARY, px, py).unwrap_err();
            assert_eq!(
                refusal.structured_content.expect("structured")["code"],
                "point_outside_window",
                "({px}, {py})"
            );
        }
    }
}
