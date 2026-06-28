//! `get_desktop_state` — full-display vision screenshot (macOS).
//!
//! Vision-only desktop capture: grabs the ENTIRE main display at native
//! pixel size (no downscale) so screen-absolute pixel picks land exactly,
//! then reports the true screen size + backing scale. No AX walk, no
//! pid/window_id — this is the capture surface for `capture_scope="desktop"`
//! GUI loops where the agent drives `click(x,y)` / `scroll(x,y)` against
//! screen-absolute coordinates.
//!
//! Mirrors `get_window_state.rs`'s vision ToolResult shape: an `image_png`
//! content part (or a written-out file path), a text summary line, and a
//! `structuredContent` object.

use async_trait::async_trait;
use cua_driver_core::{protocol::{ToolResult, Content}, tool::{Tool, ToolDef}};
use serde_json::Value;

use super::get_screen_size::main_screen_size;

pub struct GetDesktopStateTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "get_desktop_state".into(),
        description: "Capture a full-display vision screenshot in true screen pixels \
            (no downscale), for scope=\"desktop\" GUI loops where the agent then \
            drives click(x,y, scope=\"desktop\") with no pid/window_id. Returns the PNG at native \
            display resolution plus the true screen size and backing scale factor so \
            screen-absolute pixel picks land exactly. Vision-only: no AX tree walk."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "session": { "type": "string", "description": "Optional session id." },
                "screenshot_out_file": { "type": "string", "description": "Write PNG here instead of base64." }
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
    fn def(&self) -> &ToolDef { def() }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;

        let screenshot_out_file = args.opt_str("screenshot_out_file").map(|s| {
            // Expand ~ prefix (mirrors get_window_state).
            if s.starts_with("~/") {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{home}/{}", &s[2..])
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
        // blocking screencapture subprocess off the async runtime.
        let out_file = screenshot_out_file.clone();
        let res = tokio::task::spawn_blocking(
            move || -> anyhow::Result<(Option<String>, Option<String>, u32, u32)> {
                use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
                let png = crate::capture::screenshot_display_bytes()?;
                let (w, h) = crate::capture::png_dimensions(&png)?;
                if let Some(ref path) = out_file {
                    std::fs::write(path, &png)?;
                    Ok((None, Some(path.clone()), w, h))
                } else {
                    Ok((Some(BASE64.encode(&png)), None, w, h))
                }
            },
        )
        .await;

        let (b64_opt, file_path, screenshot_width, screenshot_height) = match res {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return ToolResult::error(format!("Desktop screenshot failed: {e}")),
            Err(e) => return ToolResult::error(format!("Desktop screenshot task error: {e}")),
        };

        let mut content: Vec<Content> = Vec::new();
        if let Some(b64) = b64_opt {
            content.push(Content::image_png(b64));
        }
        let summary = format!(
            "desktop screenshot {screenshot_width}x{screenshot_height} px \
             (screen {screen_width}x{screen_height} pts @ {scale_factor}x)"
        );
        content.push(Content::text(summary));

        let mut structured = serde_json::json!({
            "platform": "macos",
            "screenshot_width": screenshot_width,
            "screenshot_height": screenshot_height,
            "screen_width": screen_width,
            "screen_height": screen_height,
            "scale_factor": scale_factor,
            "screenshot_mime_type": "image/png",
        });
        if let Some(ref fp) = file_path {
            structured["screenshot_file_path"] = serde_json::json!(fp);
        }

        ToolResult { content, is_error: None, structured_content: Some(structured) }
    }
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
        assert!(!props.contains_key("window_id"), "must not accept window_id");
        assert!(!props.contains_key("capture_mode"), "must not accept capture_mode");
        assert!(props.contains_key("session"));
        assert!(props.contains_key("screenshot_out_file"));
        assert_eq!(d.input_schema["additionalProperties"], serde_json::json!(false));
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
}
