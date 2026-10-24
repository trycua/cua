use async_trait::async_trait;
use cua_driver_contract::GetDesktopStateInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_input,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{physical_screen_size, ToolState};

#[cfg(test)]
use super::impl_::is_windowless_desktop_action;

// ── get_desktop_state ─────────────────────────────────────────────────────────

/// `get_desktop_state` — full-display vision screenshot (Windows).
///
/// Vision-only desktop capture: grabs the ENTIRE primary display at native
/// physical-pixel size (no downscale) so screen-absolute pixel picks land
/// exactly, then reports the true screen size. No UIA walk, no pid/window_id —
/// this is the capture surface for actions with a primary-display desktop
/// target and screen-absolute coordinates.
///
/// Mirrors the `get_window_state` vision branch's ToolResult shape: an
/// `image_png` content part (or a written-out file path), a text summary line,
/// and a `structuredContent` object.
pub struct GetDesktopStateTool {
    pub(crate) state: Arc<ToolState>,
}
static GDS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetDesktopStateTool {
    fn def(&self) -> &ToolDef {
        GDS_DEF.get_or_init(|| ToolDef {
            name: "get_desktop_state".into(),
            description: "Capture the full display in true screen pixels, full size unless \
                `max_image_dimension` caps it. Use its PNG as the coordinate source for actions \
                whose target is {kind:\"desktop\",display_id:\"primary\"}. Returns the true \
                screen size. Vision-only: no UIA tree walk.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session." },
                    "screenshot_out_file": { "type": "string", "description": "Write PNG here instead of base64." },
                    "max_image_dimension": cua_driver_core::tool_schema::desktop_max_image_dimension_schema()
                },
                "additionalProperties": false
            }),
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::protocol::Content;
        let binding_args = args.clone();
        let input = match parse_typed_input::<GetDesktopStateInput>("get_desktop_state", args) {
            Ok(input) => input,
            Err(error) => return error,
        };
        let screenshot_out_file = input.screenshot_out_file;
        let max_image_dimension = input.max_image_dimension.filter(|cap| *cap > 0);

        // True screen geometry in physical pixels (same space as the capture).
        let (screen_width, screen_height) = physical_screen_size();

        // Capture the FULL display at native size — no resize. Run the
        // blocking GDI capture off the async runtime.
        let out_file = screenshot_out_file.clone();
        let res = tokio::task::spawn_blocking(
            move || -> anyhow::Result<(Option<String>, Option<String>, u32, u32, Vec<u8>, (u32, u32), cursor_overlay::capture_exclusion::AgentOverlayCapture)> {
                use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
                // The agent reads this image, so the Driver's own cursor and
                // session pill must not sit over the controls it is reading.
                let (png, overlay_capture) =
                    crate::capture::screenshot_display_bytes_excluding_overlay()?;
                let full = crate::capture::png_dimensions_pub(&png)?;
                // Opt-in cap; later desktop-scope pixels from the capped
                // image are mapped back at dispatch and by its capture_id.
                let png = match max_image_dimension {
                    Some(cap) if full.0.max(full.1) > cap => {
                        crate::capture::resize_png_if_needed(&png, cap)?
                    }
                    _ => png,
                };
                let (w, h) = crate::capture::png_dimensions_pub(&png)?;
                if let Some(ref path) = out_file {
                    std::fs::write(path, &png)?;
                    Ok((None, Some(path.clone()), w, h, png, full, overlay_capture))
                } else {
                    Ok((Some(BASE64.encode(&png)), None, w, h, png, full, overlay_capture))
                }
            },
        )
        .await;

        let (
            b64_opt,
            file_path,
            screenshot_width,
            screenshot_height,
            png,
            full_size,
            overlay_capture,
        ) = match res {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return ToolResult::error(format!("Desktop screenshot failed: {e}")),
            Err(e) => return ToolResult::error(format!("Desktop screenshot task error: {e}")),
        };

        let mut content: Vec<Content> = Vec::new();
        if let Some(b64) = b64_opt {
            content.push(Content::image_png(b64));
        }
        let mut summary = format!(
            "desktop screenshot {screenshot_width}x{screenshot_height} px \
             (screen {screen_width}x{screen_height} px)"
        );
        cursor_overlay::capture_exclusion::append_summary_note(&mut summary, &overlay_capture);
        content.push(Content::text(summary));

        let mut structured = json!({
            "platform": "windows",
            "display": "primary",
            "screenshot_width": screenshot_width,
            "screenshot_height": screenshot_height,
            "screen_width": screen_width,
            "screen_height": screen_height,
            "scale_factor": unsafe {
                let dpi = windows::Win32::UI::HiDpi::GetDpiForSystem();
                if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 }
            },
            "screenshot_mime_type": "image/png",
            "agent_overlay_capture": overlay_capture,
        });
        if full_size != (screenshot_width, screenshot_height) {
            structured["screenshot_original_width"] = json!(full_size.0);
            structured["screenshot_original_height"] = json!(full_size.1);
        }
        if let Some(bridge) = &self.state.capture_bridge {
            let geometry = match crate::capture_admission::CaptureGeometry::new(
                screenshot_width,
                screenshot_height,
                full_size.0,
                full_size.1,
            ) {
                Ok(geometry) => geometry,
                Err(error) => {
                    return ToolResult::error(format!(
                        "Desktop capture geometry is invalid: {error}"
                    ))
                }
            };
            match bridge.publish(
                &binding_args,
                png,
                crate::capture_admission::WindowsCaptureTarget::PrimaryDesktop,
                geometry,
            ) {
                Ok(Some(capture_id)) => structured["capture_id"] = json!(capture_id),
                Ok(None) => {}
                Err(error) => {
                    return ToolResult::error(format!(
                        "Desktop capture publication failed: {error}"
                    ))
                }
            }
        }
        if let Some(ref fp) = file_path {
            structured["screenshot_file_path"] = json!(fp);
        }

        ToolResult {
            content,
            is_error: None,
            structured_content: Some(structured),
            action_record: None,
        }
    }
}

#[cfg(test)]
mod desktop_scope_tests;
