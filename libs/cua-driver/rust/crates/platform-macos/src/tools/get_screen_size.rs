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
        name: "get_screen_size".into(),
        description: "Return every active display's global logical bounds, backing scale, and \
            main/built-in/mirroring state. Top-level width, height, and scale_factor retain the \
            main-display contract. Agents click in logical points; Retina displays commonly \
            have scale_factor 2.0. Requires no TCC permissions."
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
        let displays = active_displays();
        let Some(main) = displays.iter().find(|display| display.is_main) else {
            return ToolResult::error("No main display detected.");
        };
        let structured_displays: Vec<Value> = displays.iter().map(display_json).collect();
        ToolResult::text(format!(
            "✅ {} active display(s); main display: {}x{} points @ {}x",
            displays.len(),
            main.width,
            main.height,
            main.scale_factor
        ))
        .with_structured(serde_json::json!({
            "width": main.width,
            "height": main.height,
            "scale_factor": main.scale_factor,
            "display_count": displays.len(),
            "displays": structured_displays,
        }))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DisplayInfo {
    pub display_id: u32,
    pub x: f64,
    pub y: f64,
    pub width: i64,
    pub height: i64,
    pub pixel_width: u64,
    pub pixel_height: u64,
    pub scale_factor: f64,
    pub is_main: bool,
    pub is_builtin: bool,
    pub is_mirrored: bool,
}

pub(crate) fn active_displays() -> Vec<DisplayInfo> {
    use core_graphics::display::{CGDisplay, CGGetActiveDisplayList};

    const MAX_DISPLAYS: usize = 32;
    let mut display_ids = [0u32; MAX_DISPLAYS];
    let mut count = 0u32;
    let error = unsafe {
        CGGetActiveDisplayList(MAX_DISPLAYS as u32, display_ids.as_mut_ptr(), &mut count)
    };
    if error != 0 {
        return vec![];
    }

    display_ids[..usize::try_from(count).unwrap_or(0).min(MAX_DISPLAYS)]
        .iter()
        .copied()
        .filter(|display_id| *display_id != 0)
        .map(|display_id| {
            let display = CGDisplay::new(display_id);
            let bounds = display.bounds();
            let width = bounds.size.width.round() as i64;
            let height = bounds.size.height.round() as i64;
            DisplayInfo {
                display_id,
                x: bounds.origin.x,
                y: bounds.origin.y,
                width,
                height,
                pixel_width: display.pixels_wide(),
                pixel_height: display.pixels_high(),
                scale_factor: get_backing_scale(display_id, width),
                is_main: display.is_main(),
                is_builtin: display.is_builtin(),
                is_mirrored: display.is_in_mirror_set(),
            }
        })
        .collect()
}

fn display_json(display: &DisplayInfo) -> Value {
    serde_json::json!({
        "display_id": display.display_id,
        "bounds": {
            "x": display.x,
            "y": display.y,
            "width": display.width,
            "height": display.height
        },
        "pixel_width": display.pixel_width,
        "pixel_height": display.pixel_height,
        "scale_factor": display.scale_factor,
        "is_main": display.is_main,
        "is_builtin": display.is_builtin,
        "is_mirrored": display.is_mirrored
    })
}

/// Returns `(width_points, height_points, backing_scale_factor)` from
/// CoreGraphics, safe to call from any thread.
pub(crate) fn main_screen_size() -> Option<(i64, i64, f64)> {
    active_displays()
        .into_iter()
        .find(|display| display.is_main)
        .map(|display| (display.width, display.height, display.scale_factor))
}

/// Estimate backing scale by comparing the display's pixel mode width to its
/// logical (CoreGraphics) bounds width.
pub(crate) fn get_backing_scale(display_id: u32, logical_w: i64) -> f64 {
    use core_graphics::display::CGDisplayPixelsWide;
    let pixel_w = unsafe { CGDisplayPixelsWide(display_id) } as i64;
    if pixel_w > 0 && logical_w > 0 {
        let ratio = pixel_w as f64 / logical_w as f64;
        // Round to nearest 0.5 to avoid floating point noise.
        (ratio * 2.0).round() / 2.0
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_has_no_pid_or_window_id_and_is_read_only() {
        let definition = def();
        assert!(definition.read_only);
        assert!(!definition.destructive);
        assert!(definition.idempotent);
        assert!(!definition.open_world);

        let properties = definition.input_schema["properties"].as_object().unwrap();
        assert!(!properties.contains_key("pid"));
        assert!(!properties.contains_key("window_id"));
        assert!(properties.contains_key("session"));
    }

    #[test]
    fn display_json_preserves_global_bounds_and_identity() {
        let display = DisplayInfo {
            display_id: 9,
            x: -1728.0,
            y: 120.0,
            width: 1728,
            height: 1117,
            pixel_width: 3456,
            pixel_height: 2234,
            scale_factor: 2.0,
            is_main: false,
            is_builtin: true,
            is_mirrored: false,
        };
        let value = display_json(&display);
        assert_eq!(value["display_id"], 9);
        assert_eq!(value["bounds"]["x"], -1728.0);
        assert_eq!(value["bounds"]["y"], 120.0);
        assert_eq!(value["is_builtin"], true);
        assert_eq!(value["is_main"], false);
    }
}
