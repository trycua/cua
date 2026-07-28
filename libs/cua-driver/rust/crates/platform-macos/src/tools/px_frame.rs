//! The one place that turns window-local screenshot pixels into screen
//! coordinates for the pixel rungs (`click`, `double_click`, `right_click`,
//! `drag`, `scroll`).
//!
//! Issue #2237: each of those tools carried a byte-identical copy of this
//! translation, and every copy silently reinterpreted window-local pixels as
//! screen-absolute when `window_bounds_by_id` returned `None`. A dismissed
//! Open/Save panel's coordinates then landed on whatever was behind it — a
//! misclick reported as success. There is no safe fallback here: without the
//! window's origin there is nothing to add the local point to, so a missing or
//! stale window is a refusal.

use cua_driver_core::protocol::ToolResult;

use crate::windows::WindowBounds;

/// A window's screen origin plus the physical-pixels-per-logical-point scale
/// of its `screencapture` output.
#[derive(Debug, Clone)]
pub struct WindowPxFrame {
    pub bounds: WindowBounds,
    /// Physical capture pixels per logical point (1.0 non-Retina, 2.0 Retina).
    pub scale: f64,
}

impl WindowPxFrame {
    /// Window-local capture pixels → `(screen_x, screen_y, local_pt_x,
    /// local_pt_y)`. The local-point pair is what
    /// `CGEventSetWindowLocation` needs for background delivery.
    pub fn to_screen(&self, cx: f64, cy: f64) -> (f64, f64, f64, f64) {
        let lx = cx / self.scale;
        let ly = cy / self.scale;
        (self.bounds.x + lx, self.bounds.y + ly, lx, ly)
    }
}

/// Why a window-local pixel action cannot be translated.
#[derive(Debug, Clone, PartialEq)]
pub enum PxFrameError {
    /// WindowServer has no usable frame for this id: it was closed, the id is
    /// stale or fabricated, or the record carries degenerate `0×0` bounds
    /// (`windows.rs`'s missing-`kCGWindowBounds` default), which would make
    /// the window origin indistinguishable from the screen origin.
    WindowNotFound { window_id: u32 },
    /// The current capture could not be obtained or decoded, so its pixel
    /// coordinate system cannot be proven against the WindowServer frame.
    CaptureUnavailable { window_id: u32, reason: String },
    /// The capture dimensions are not a coherent 1×/2× representation of the
    /// requested WindowServer frame. Dispatching with this transform would
    /// recreate #2237's wrong-surface misclick.
    FrameMismatch {
        window_id: u32,
        bounds_width: f64,
        bounds_height: f64,
        capture_width: u32,
        capture_height: u32,
        scale_x: f64,
        scale_y: f64,
    },
}

/// Validate that a raw window capture and its WindowServer bounds describe one
/// coordinate frame, returning the physical-pixels-per-point backing scale.
///
/// macOS window captures are 1× or 2×. Small rounding differences are allowed,
/// but arbitrary ratios are not: the original #2237 failure paired a parent
/// window screenshot with panel bounds and would otherwise look like a
/// plausible-but-wrong ~2.2× scale.
pub fn validate_capture_frame(
    window_id: u32,
    bounds: &WindowBounds,
    capture_px_w: u32,
    capture_px_h: u32,
) -> Result<f64, PxFrameError> {
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return Err(PxFrameError::WindowNotFound { window_id });
    }
    let scale_x = capture_px_w as f64 / bounds.width;
    let scale_y = capture_px_h as f64 / bounds.height;
    let nearest = if (scale_x - 1.0).abs() <= (scale_x - 2.0).abs() {
        1.0
    } else {
        2.0
    };
    let coherent = scale_x.is_finite()
        && scale_y.is_finite()
        && (scale_x - scale_y).abs() <= 0.03
        && (scale_x - nearest).abs() <= 0.03
        && (scale_y - nearest).abs() <= 0.03;
    if coherent {
        Ok(nearest)
    } else {
        Err(PxFrameError::FrameMismatch {
            window_id,
            bounds_width: bounds.width,
            bounds_height: bounds.height,
            capture_width: capture_px_w,
            capture_height: capture_px_h,
            scale_x,
            scale_y,
        })
    }
}

/// Resolve `window_id`'s screen origin and capture scale. Blocking: enumerates
/// CGWindowList and takes one window capture to measure the scale.
pub fn resolve_window_px_frame(window_id: u32) -> Result<WindowPxFrame, PxFrameError> {
    let bounds = crate::windows::window_bounds_by_id(window_id)
        .filter(|b| b.width > 0.0 && b.height > 0.0)
        .ok_or(PxFrameError::WindowNotFound { window_id })?;
    // A window-local point is meaningful only when the current capture proves
    // the same frame and scale. Never guess 1× on capture failure.
    let png = crate::capture::screenshot_window_bytes(window_id).map_err(|e| {
        PxFrameError::CaptureUnavailable {
            window_id,
            reason: e.to_string(),
        }
    })?;
    let (pw, ph) =
        crate::capture::png_dimensions(&png).map_err(|e| PxFrameError::CaptureUnavailable {
            window_id,
            reason: e.to_string(),
        })?;
    let scale = validate_capture_frame(window_id, &bounds, pw, ph)?;
    Ok(WindowPxFrame { bounds, scale })
}

/// The shared refusal for a pixel action whose window cannot be framed.
pub fn refusal(error: &PxFrameError) -> ToolResult {
    match error {
        PxFrameError::WindowNotFound { window_id } => ToolResult::error(format!(
            "window_id {window_id} has no live frame, so window-local pixels cannot be \
             translated to screen coordinates. Refusing to dispatch — treating them as \
             screen-absolute would act on whatever is behind the closed window. Call \
             list_windows for a current window_id, then re-snapshot with get_window_state."
        ))
        .with_structured(error_structured(error)),
        PxFrameError::CaptureUnavailable { window_id, reason } => ToolResult::error(format!(
            "window_id {window_id}'s current capture is unavailable ({reason}), so its \
                 pixel coordinate frame cannot be verified. Refusing to dispatch."
        ))
        .with_structured(error_structured(error)),
        PxFrameError::FrameMismatch {
            window_id,
            bounds_width,
            bounds_height,
            capture_width,
            capture_height,
            scale_x,
            scale_y,
        } => ToolResult::error(format!(
            "window_id {window_id}'s capture is {capture_width}x{capture_height}, but its \
             WindowServer bounds are {bounds_width:.2}x{bounds_height:.2} points \
             (scale_x={scale_x:.4}, scale_y={scale_y:.4}). These are not one coherent \
             1x/2x frame. Refusing to dispatch."
        ))
        .with_structured(error_structured(error)),
    }
}

/// Structured form shared by action refusals and a successful state snapshot
/// whose optional screenshot could not be proven. The latter must keep its AX
/// payload rather than turning a capture problem into a total perception
/// failure.
pub fn error_structured(error: &PxFrameError) -> serde_json::Value {
    match error {
        PxFrameError::WindowNotFound { window_id } => serde_json::json!({
            "code": "px_window_not_found",
            "window_id": window_id,
            "suggestion": "refusing to interpret window-local pixels as screen-absolute; \
                           call list_windows for a current window_id"
        }),
        PxFrameError::CaptureUnavailable { window_id, reason } => serde_json::json!({
            "code": "px_capture_unavailable",
            "window_id": window_id,
            "reason": reason,
            "suggestion": "re-snapshot the window after capture permission/state recovers"
        }),
        PxFrameError::FrameMismatch {
            window_id,
            bounds_width,
            bounds_height,
            capture_width,
            capture_height,
            scale_x,
            scale_y,
        } => serde_json::json!({
            "code": "px_frame_mismatch",
            "window_id": window_id,
            "window_bounds": {
                "width": bounds_width,
                "height": bounds_height
            },
            "capture_size": {
                "width": capture_width,
                "height": capture_height
            },
            "scale_x": scale_x,
            "scale_y": scale_y,
            "suggestion": "re-list and re-snapshot the exact window; do not reuse coordinates \
                           from a different surface"
        }),
    }
}

/// Resolve the frame off the async path, mapping both the task error and the
/// refusal onto a `ToolResult` the caller can return directly.
pub async fn resolve_or_refuse(window_id: u32) -> Result<WindowPxFrame, ToolResult> {
    match tokio::task::spawn_blocking(move || resolve_window_px_frame(window_id)).await {
        Ok(Ok(frame)) => Ok(frame),
        Ok(Err(e)) => Err(refusal(&e)),
        Err(e) => Err(ToolResult::error(format!(
            "window frame lookup for window_id {window_id} failed: {e}. Not dispatching."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(scale: f64) -> WindowPxFrame {
        WindowPxFrame {
            bounds: WindowBounds {
                x: 100.0,
                y: 580.0,
                width: 500.0,
                height: 500.0,
            },
            scale,
        }
    }

    #[test]
    fn resolves_1x_window_local_to_screen() {
        let (sx, sy, lx, ly) = frame(1.0).to_screen(30.0, 40.0);
        assert_eq!((sx, sy), (130.0, 620.0));
        assert_eq!((lx, ly), (30.0, 40.0));
    }

    #[test]
    fn resolves_2x_retina_scale() {
        // A point 60 capture px in is 30 pt in.
        let (sx, sy, lx, ly) = frame(2.0).to_screen(60.0, 80.0);
        assert_eq!((sx, sy), (130.0, 620.0));
        assert_eq!((lx, ly), (30.0, 40.0));
    }

    #[test]
    fn backing_scale_detects_retina_capture() {
        let b = frame(1.0).bounds;
        assert_eq!(validate_capture_frame(11, &b, 1000, 1000).unwrap(), 2.0);
    }

    #[test]
    fn backing_scale_is_one_for_point_sized_capture() {
        let b = frame(1.0).bounds;
        assert_eq!(validate_capture_frame(11, &b, 500, 500).unwrap(), 1.0);
    }

    #[test]
    fn backing_scale_never_divides_by_zero_bounds() {
        // windows.rs defaults absent kCGWindowBounds to 0×0.
        let mut b = frame(1.0).bounds;
        b.width = 0.0;
        assert_eq!(
            validate_capture_frame(11, &b, 1000, 1000),
            Err(PxFrameError::WindowNotFound { window_id: 11 })
        );
    }

    #[test]
    fn capture_narrower_than_bounds_is_a_mismatch() {
        let b = frame(1.0).bounds;
        assert!(matches!(
            validate_capture_frame(11, &b, 250, 250),
            Err(PxFrameError::FrameMismatch { .. })
        ));
    }

    #[test]
    fn parent_capture_cannot_masquerade_as_a_panel_scale() {
        let bounds = WindowBounds {
            x: 72.0,
            y: 88.0,
            width: 880.0,
            height: 448.0,
        };
        assert!(matches!(
            validate_capture_frame(55, &bounds, 1920, 960),
            Err(PxFrameError::FrameMismatch { .. })
        ));
    }

    /// Locks the exact regression: a stale window_id used to fall through to
    /// "treat x,y as screen coordinates".
    #[test]
    fn missing_window_refuses_instead_of_treating_local_as_screen_px() {
        let refused = refusal(&PxFrameError::WindowNotFound { window_id: 67340 });
        assert_eq!(refused.is_error, Some(true));
        let structured = refused.structured_content.expect("structured refusal");
        assert_eq!(structured["code"], "px_window_not_found");
        assert_eq!(structured["window_id"], 67340);
        assert!(
            structured["suggestion"]
                .as_str()
                .unwrap()
                .contains("list_windows"),
            "the refusal must name the retry path"
        );
    }
}
