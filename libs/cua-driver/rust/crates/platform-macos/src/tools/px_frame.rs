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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PxFrameError {
    /// WindowServer has no usable frame for this id: it was closed, the id is
    /// stale or fabricated, or the record carries degenerate `0×0` bounds
    /// (`windows.rs`'s missing-`kCGWindowBounds` default), which would make
    /// the window origin indistinguishable from the screen origin.
    WindowNotFound { window_id: u32 },
}

/// Derive the capture's backing scale by comparing its physical width to the
/// window's logical width.
///
/// Pure so the Retina arithmetic is testable without a display. Falls back to
/// `1.0` whenever the comparison is not meaningful — a capture that is not
/// wider than the logical bounds is already in points.
pub fn backing_scale_from_capture(capture_px_w: u32, logical_w: f64) -> f64 {
    let pw = capture_px_w as f64;
    if logical_w > 0.0 && pw > logical_w {
        pw / logical_w
    } else {
        1.0
    }
}

/// Resolve `window_id`'s screen origin and capture scale. Blocking: enumerates
/// CGWindowList and takes one window capture to measure the scale.
pub fn resolve_window_px_frame(window_id: u32) -> Result<WindowPxFrame, PxFrameError> {
    let bounds = crate::windows::window_bounds_by_id(window_id)
        .filter(|b| b.width > 0.0 && b.height > 0.0)
        .ok_or(PxFrameError::WindowNotFound { window_id })?;
    // The capture is only used to measure pixels-per-point. A capture failure
    // (e.g. screen-recording denied) is not a reason to refuse the action: the
    // window origin is known, so fall back to 1.0 as the pre-existing code did.
    let scale = match crate::capture::screenshot_window_bytes(window_id) {
        Ok(png) => match crate::capture::png_dimensions(&png) {
            Ok((pw, _)) => backing_scale_from_capture(pw, bounds.width),
            Err(_) => 1.0,
        },
        Err(_) => 1.0,
    };
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
        .with_structured(serde_json::json!({
            "code": "px_window_not_found",
            "window_id": window_id,
            "suggestion": "refusing to interpret window-local pixels as screen-absolute; \
                           call list_windows for a current window_id"
        })),
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
        assert_eq!(backing_scale_from_capture(1000, 500.0), 2.0);
    }

    #[test]
    fn backing_scale_is_one_for_point_sized_capture() {
        assert_eq!(backing_scale_from_capture(500, 500.0), 1.0);
    }

    #[test]
    fn backing_scale_never_divides_by_zero_bounds() {
        // windows.rs defaults absent kCGWindowBounds to 0×0.
        assert_eq!(backing_scale_from_capture(1000, 0.0), 1.0);
    }

    #[test]
    fn backing_scale_ignores_a_capture_narrower_than_bounds() {
        // A clipped/off-Space capture must not produce a sub-1 scale that
        // would inflate every translated coordinate.
        assert_eq!(backing_scale_from_capture(250, 500.0), 1.0);
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
