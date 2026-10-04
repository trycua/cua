use async_trait::async_trait;
use cua_driver_contract::GetScreenSizeInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_input,
};
use serde_json::Value;

pub struct GetScreenSizeTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        // Matches `GetScreenSizeTool.swift` description verbatim.
        name: "get_screen_size".into(),
        description: "Return the logical size of the main display in points plus its backing \
            scale factor. Agents click in points; Retina displays have scale_factor 2.0. \
            Requires no TCC permissions."
            .into(),
        input_schema: serde_json::json!({"type":"object","properties":{
            "session": cua_driver_core::tool_schema::session_schema()
        },"additionalProperties":false}),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for GetScreenSizeTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        if let Err(result) = parse_typed_input::<GetScreenSizeInput>("get_screen_size", args) {
            return result;
        }
        match main_screen_size() {
            Some((w, h, scale)) => {
                // Matches Swift text format 1:1.
                ToolResult::text(format!("✅ Main display: {w}x{h} points @ {scale}x"))
                    .with_structured(serde_json::json!({
                        "width": w, "height": h, "scale_factor": scale,
                    }))
            }
            None => ToolResult::error("No main display detected."),
        }
    }
}

/// Returns `(width_points, height_points, backing_scale_factor)` from
/// CoreGraphics — safe to call from any thread (no AppKit main-thread requirement).
///
/// The previous NSScreen-based implementation required `MainThreadMarker::new()`
/// which always returns `None` on async tokio threads, causing the tool to
/// return an error even when a display is attached.
pub(crate) fn main_screen_size() -> Option<(i64, i64, f64)> {
    use core_graphics::display::{CGDisplayBounds, CGMainDisplayID};

    // SAFETY: CGMainDisplayID / CGDisplayBounds are thread-safe CG APIs.
    let display_id = unsafe { CGMainDisplayID() };
    if display_id == 0 {
        return None;
    }
    let bounds = unsafe { CGDisplayBounds(display_id) };
    let w = bounds.size.width as i64;
    let h = bounds.size.height as i64;
    if w == 0 || h == 0 {
        return None;
    }

    let scale = get_backing_scale(display_id);
    Some((w, h, scale))
}

/// Return the current display mode's backing-pixel to point ratio.
pub(crate) fn get_backing_scale(display_id: u32) -> f64 {
    use core_graphics::display::CGDisplay;

    let Some(mode) = CGDisplay::new(display_id).display_mode() else {
        return 1.0;
    };
    backing_scale_from_widths(mode.width(), mode.pixel_width())
}

fn backing_scale_from_widths(point_width: u64, pixel_width: u64) -> f64 {
    if point_width == 0 || pixel_width == 0 {
        return 1.0;
    }
    let ratio = pixel_width as f64 / point_width as f64;
    // Round to nearest 0.5 to avoid floating point noise.
    (ratio * 2.0).round() / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_graphics::display::CGDisplay;

    #[test]
    #[ignore = "requires an attached macOS display; run explicitly on a native Retina host"]
    fn main_display_scale_matches_current_mode() {
        let display = CGDisplay::main();
        let mode = display
            .display_mode()
            .expect("main display should expose its current mode");
        let point_width = mode.width();
        let pixel_width = mode.pixel_width();
        assert!(point_width > 0, "display mode should have a point width");
        assert!(pixel_width > 0, "display mode should have a pixel width");
        assert!(
            pixel_width > point_width,
            "native Retina validation requires more backing pixels than points; got {pixel_width}px for {point_width}pt"
        );

        let expected = ((pixel_width as f64 / point_width as f64) * 2.0).round() / 2.0;
        let actual = get_backing_scale(display.id);

        assert_eq!(
            actual, expected,
            "backing scale should use the current mode's pixel and point widths"
        );
    }

    #[test]
    fn display_mode_widths_preserve_rounding_and_fallbacks() {
        assert_eq!(backing_scale_from_widths(1920, 3840), 2.0);
        assert_eq!(backing_scale_from_widths(1920, 2880), 1.5);
        assert_eq!(backing_scale_from_widths(0, 3840), 1.0);
        assert_eq!(backing_scale_from_widths(1920, 0), 1.0);
    }
}
