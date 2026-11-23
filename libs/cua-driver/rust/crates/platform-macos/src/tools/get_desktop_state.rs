// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `get_desktop_state` — full-display vision screenshot (macOS).
//!
//! Vision-only desktop capture: grabs the ENTIRE main display at native
//! pixel size (no downscale) so screen-absolute pixel picks land exactly,
//! then reports the true screen size + backing scale. No AX walk, no
//! pid/window_id. This is the capture surface for actions with a primary-display
//! desktop target and screen-absolute coordinates.
//!
//! Mirrors `get_window_state.rs`'s vision ToolResult shape: an `image_png`
//! content part (or a written-out file path), a text summary line, and a
//! `structuredContent` object.

use async_trait::async_trait;
use cua_driver_contract::GetDesktopStateInput;
use cua_driver_core::{
    protocol::{Content, ToolResult},
    tool::{Tool, ToolDef},
    tool_args::parse_typed_input,
};
use serde_json::Value;
use std::sync::Arc;

use super::{get_screen_size::main_screen_size, ToolState};

pub struct GetDesktopStateTool {
    state: Arc<ToolState>,
}

impl GetDesktopStateTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "get_desktop_state".into(),
        description: "Capture the full display. The long edge is capped at \
            `max_image_dimension` (the configured default unless given); pixels of the \
            returned image map back to the screen through its capture. Use its PNG as the coordinate source for actions \
            whose target is {kind:\"desktop\",display_id:\"primary\"}. Returns the true \
            screen size and backing scale factor. Vision-only: no AX tree walk. \
            `exclude_window_ids` leaves the given windows out of the image, or the call fails."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "session": cua_driver_core::tool_schema::session_schema(),
                "screenshot_out_file": { "type": "string", "description": "Write PNG here instead of base64." },
                "max_image_dimension": cua_driver_core::tool_schema::desktop_max_image_dimension_schema(),
                "exclude_window_ids": cua_driver_core::tool_schema::desktop_exclude_window_ids_schema()
            },
            "additionalProperties": false
        }),
        read_only: true,
        destructive: false,
        idempotent: false,
        open_world: false,
    })
}

#[async_trait]
impl Tool for GetDesktopStateTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let capture_args = args.clone();
        let input = match parse_typed_input::<GetDesktopStateInput>("get_desktop_state", args) {
            Ok(input) => input,
            Err(result) => return result,
        };
        // Call arg > session override > global config, as get_window_state.
        let max_image_dimension = input.max_image_dimension.or_else(|| {
            use cua_driver_core::tool_args::ArgsExt;
            let session_id = capture_args.opt_str("_session_id");
            let cfg = self.state.config.read().unwrap();
            Some(
                self.state
                    .session_config
                    .effective_max_image_dimension(session_id.as_deref(), &cfg),
            )
        });
        let max_image_dimension = max_image_dimension.filter(|cap| *cap > 0);
        let exclude_window_ids = match parse_exclude_window_ids(input.exclude_window_ids.as_deref())
        {
            Ok(ids) => ids,
            Err(result) => return result,
        };
        let screenshot_out_file = input.screenshot_out_file.map(|s| {
            // Expand ~ prefix (mirrors get_window_state).
            if let Some(relative) = s.strip_prefix("~/") {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{home}/{relative}")
            } else {
                s
            }
        });

        // True screen geometry (points + backing scale). Safe off the main thread.
        let (screen_width, screen_height, scale_factor) = match main_screen_size() {
            Some(t) => t,
            None => return ToolResult::error("No main display detected."),
        };

        // Capture the FULL display at native size — no resize. Run the
        // blocking display capture off the async runtime.
        let out_file = screenshot_out_file.clone();
        let excluded = exclude_window_ids.clone();
        let res = tokio::task::spawn_blocking(
            move || -> anyhow::Result<(
                Vec<u8>,
                Option<String>,
                u32,
                u32,
                (u32, u32),
                cursor_overlay::capture_exclusion::AgentOverlayCapture,
            )> {
                // The agent reads this image, so the Driver's own cursor and
                // session pill must not sit over the controls it is reading.
                let (png, overlay_capture) =
                    crate::capture::screenshot_display_bytes_excluding_overlay(&excluded)?;
                let full = crate::capture::png_dimensions(&png)?;
                // Capped image: later desktop-scope pixels from it are
                // mapped back at dispatch and by its capture_id.
                let png = match max_image_dimension {
                    Some(cap) if full.0.max(full.1) > cap => {
                        crate::capture::resize_png_if_needed(&png, cap)?
                    }
                    _ => png,
                };
                let (w, h) = crate::capture::png_dimensions(&png)?;
                if let Some(ref path) = out_file {
                    std::fs::write(path, &png)?;
                    Ok((png, Some(path.clone()), w, h, full, overlay_capture))
                } else {
                    Ok((png, None, w, h, full, overlay_capture))
                }
            },
        )
        .await;

        let (png, file_path, screenshot_width, screenshot_height, full_size, overlay_capture) =
            match res {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    let result = ToolResult::error(format!("Desktop screenshot failed: {e}"));
                    return match e.downcast_ref::<crate::capture::WindowExclusionError>() {
                        Some(exclusion) => result.with_structured(serde_json::json!({
                            "code": exclusion.code(),
                            "tool": "get_desktop_state",
                            "detail": exclusion.to_string(),
                        })),
                        None => result,
                    };
                }
                Err(e) => return ToolResult::error(format!("Desktop screenshot task error: {e}")),
            };

        let native_width = match u32::try_from(screen_width) {
            Ok(width) => width,
            Err(_) => return ToolResult::error("Desktop screen width is out of range."),
        };
        let native_height = match u32::try_from(screen_height) {
            Ok(height) => height,
            Err(_) => return ToolResult::error("Desktop screen height is out of range."),
        };
        let capture_id = match self.state.capture_bindings.publish_desktop(
            &capture_args,
            png.clone(),
            (screenshot_width, screenshot_height),
            (native_width, native_height),
        ) {
            Ok(capture_id) => capture_id,
            Err(error) => return error,
        };

        let mut content: Vec<Content> = Vec::new();
        if file_path.is_none() {
            use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
            content.push(Content::image_png(BASE64.encode(&png)));
        }
        let mut summary = format!(
            "desktop screenshot {screenshot_width}x{screenshot_height} px \
             (screen {screen_width}x{screen_height} pts @ {scale_factor}x)"
        );
        cursor_overlay::capture_exclusion::append_summary_note(&mut summary, &overlay_capture);
        content.push(Content::text(summary));

        let mut structured = serde_json::json!({
            "platform": "macos",
            "display": "primary",
            "screenshot_width": screenshot_width,
            "screenshot_height": screenshot_height,
            "screen_width": screen_width,
            "screen_height": screen_height,
            "scale_factor": scale_factor,
            "screenshot_mime_type": "image/png",
            "capture_id": capture_id,
            "agent_overlay_capture": overlay_capture,
        });
        if full_size != (screenshot_width, screenshot_height) {
            structured["screenshot_original_width"] = serde_json::json!(full_size.0);
            structured["screenshot_original_height"] = serde_json::json!(full_size.1);
        }
        if let Some(ref fp) = file_path {
            structured["screenshot_file_path"] = serde_json::json!(fp);
        }
        if !exclude_window_ids.is_empty() {
            structured["excluded_window_ids"] = serde_json::json!(exclude_window_ids);
        }

        ToolResult {
            content,
            is_error: None,
            structured_content: Some(structured),
            action_record: None,
        }
    }
}

/// macOS window ids are `CGWindowID`s; a wider value names no window.
fn parse_exclude_window_ids(ids: Option<&[u64]>) -> Result<Vec<u32>, ToolResult> {
    ids.unwrap_or_default()
        .iter()
        .map(|&id| {
            u32::try_from(id).map_err(|_| {
                ToolResult::error(format!(
                    "get_desktop_state: exclude_window_ids entry {id} is not a macOS window id"
                ))
                .with_structured(serde_json::json!({
                    "code": "invalid_arguments",
                    "tool": "get_desktop_state",
                    "detail": format!("exclude_window_ids entry {id} exceeds u32"),
                }))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_has_no_pid_or_window_id_and_is_read_only() {
        let d = def();
        assert!(d.read_only, "get_desktop_state must be read_only");
        assert!(!d.destructive);
        assert!(!d.idempotent);
        assert!(!d.open_world);

        let props = d.input_schema["properties"].as_object().unwrap();
        assert!(!props.contains_key("pid"), "must not accept pid");
        assert!(
            !props.contains_key("window_id"),
            "must not accept window_id"
        );
        assert!(
            !props.contains_key("capture_mode"),
            "must not accept capture_mode"
        );
        assert!(props.contains_key("session"));
        assert!(props.contains_key("screenshot_out_file"));
        assert!(props.contains_key("exclude_window_ids"));
        assert_eq!(
            d.input_schema["additionalProperties"],
            serde_json::json!(false)
        );
    }

    #[test]
    fn description_mentions_full_and_screen_or_display() {
        let desc = def().description.to_lowercase();
        assert!(desc.contains("full"), "description must mention 'full'");
        assert!(
            desc.contains("screen") || desc.contains("display"),
            "description must mention 'screen' or 'display'"
        );
    }

    #[test]
    fn exclude_window_ids_refuses_ids_wider_than_a_cg_window_id() {
        assert_eq!(parse_exclude_window_ids(None).unwrap(), Vec::<u32>::new());
        assert_eq!(
            parse_exclude_window_ids(Some(&[7, 42])).unwrap(),
            vec![7, 42]
        );
        let refused = parse_exclude_window_ids(Some(&[u64::from(u32::MAX) + 1])).unwrap_err();
        assert_eq!(
            refused.structured_content.unwrap()["code"],
            "invalid_arguments"
        );
    }
}
