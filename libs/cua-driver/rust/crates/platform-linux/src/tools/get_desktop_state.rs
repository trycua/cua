use async_trait::async_trait;
use cua_driver_contract::GetDesktopStateInput;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
    tool_args::parse_typed_input,
};
use serde_json::{json, Value};
use std::sync::Arc;

use super::impl_::{
    normalize_desktop_capture_for_action_frame, window_record_json, x11_screen_size, ToolState,
};

#[cfg(test)]
use super::impl_::maps_indicate_synthetic_pointer_dropped;

// ── get_desktop_state ─────────────────────────────────────────────────────────

pub struct GetDesktopStateTool {
    pub(crate) state: Arc<ToolState>,
}
static GDS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetDesktopStateTool {
    fn def(&self) -> &ToolDef {
        GDS_DEF.get_or_init(|| ToolDef {
            name: "get_desktop_state".into(),
            description: "Capture the full display in the desktop action coordinate frame. \
                Use the returned PNG directly as the coordinate source for actions whose target is \
                {kind:\"desktop\",display_id:\"primary\"}. No AT-SPI walk.".into(),
            input_schema: json!({"type":"object","properties":{
                "session":{"type":"string","description":"For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session."},
                "screenshot_out_file":{"type":"string","description":"Write PNG here instead of base64."},
                "max_image_dimension": cua_driver_core::tool_schema::desktop_max_image_dimension_schema()
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let capture_args = args.clone();
        let input = match parse_typed_input::<GetDesktopStateInput>("get_desktop_state", args) {
            Ok(input) => input,
            Err(result) => return result,
        };
        let out_file = input.screenshot_out_file;
        let max_image_dimension = input.max_image_dimension;
        let capture_service = self.state.capture_service.clone();

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            // Capture the full display at native size first. When the
            // compositor consumes logical input coordinates, normalize the
            // image below so screenshot pixels still land exactly.
            //
            // The agent reads this image, so the Driver's own cursor and
            // session pill are hidden around the grab (or the limitation is
            // reported) instead of being baked over the controls it reads.
            // Hyprland: read the desktop frame once. The screenshot, its action
            // size and the window geometry below all use it.
            let hyprland_plan =
                if crate::wayland::is_wayland() && crate::wayland::hyprland::is_session() {
                    Some(crate::wayland::hyprland::desktop_capture_plan()?)
                } else {
                    None
                };
            // niri: the single output's frame, read before the capture and
            // compared after it.
            let niri_frame = (crate::wayland::is_wayland() && crate::wayland::niri::is_session())
                .then(|| crate::wayland::niri::desktop_snapshot().map(|snapshot| snapshot.frame))
                .transpose()?;
            let (native_png, overlay_capture) =
                cursor_overlay::capture_exclusion::capture_excluding_overlays(
                    &crate::overlay_capture::OverlayExcluder,
                    |hidden| {
                        // A multi-monitor layout is composed from the frame
                        // read above, so the image is exactly that frame.
                        let png = match &hyprland_plan {
                            Some(plan) if plan.composite => {
                                crate::wayland::hyprland::compose_desktop_capture(&plan.frame)?
                            }
                            _ => crate::capture::screenshot_display_bytes()?,
                        };
                        Ok::<_, anyhow::Error>(crate::overlay_capture::verify_hidden_capture(
                            png, hidden,
                        ))
                    },
                )?;
            let (native_w, native_h) = crate::capture::png_dimensions_pub(&native_png)?;
            // True screen size. On a pure-Wayland session (native backend
            // opted in, no X11 DISPLAY) the capture above came from the
            // wlroots `zwlr_screencopy` cascade, whose full-display buffer is
            // the whole output at native (physical) pixels — so the PNG
            // dimensions ARE the true screen size. Querying the X11 root
            // window here would fail with "$DISPLAY variable not set" and
            // abort the tool even though the screenshot already succeeded.
            // Only fall back to the X11 root-window geometry off Wayland, so
            // the X11 / XWayland path is unchanged. See #2017 / Sway testing.
            let hyprland_frame = hyprland_plan.map(|plan| plan.frame);
            let (screen_w, screen_h) = if let Some(frame) = &hyprland_frame {
                (frame.width, frame.height)
            } else if crate::wayland::is_wayland() {
                crate::capture_action_frame::desktop_action_dimensions((native_w, native_h))?
            } else {
                x11_screen_size()?
            };
            let (png, shot_w, shot_h, scale_factor) =
                normalize_desktop_capture_for_action_frame(native_png, screen_w, screen_h)?;
            // Opt-in long-edge cap: the full-size capture is the desktop
            // action frame; a capped image is mapped back at dispatch
            // (cua_driver_core::desktop_capture_scale) and by its capture_id.
            let (png, shot_w, shot_h) = match max_image_dimension.filter(|cap| *cap > 0) {
                Some(cap) if shot_w.max(shot_h) > cap => {
                    let png = crate::capture::resize_png_if_needed(&png, cap)?;
                    let (w, h) = crate::capture::png_dimensions_pub(&png)?;
                    (png, w, h)
                }
                _ => (png, shot_w, shot_h),
            };
            // Optional: write PNG to disk instead of returning base64.
            let written = if let Some(path) = out_file.as_deref() {
                std::fs::write(path, &png)?;
                Some(path.to_string())
            } else {
                None
            };
            use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
            let b64 = if written.is_some() {
                None
            } else {
                Some(B64.encode(&png))
            };
            // Which pid owns which window is what the agent needs before its
            // first click; without it the first action guesses `pid: 1`.
            let mut windows = crate::wayland::list_windows_dispatch(None);
            windows
                .retain(|w| w.is_on_screen && w.pid.is_some_and(crate::proc_fs::is_process_live));
            if let Some(frame) = &hyprland_frame {
                // Window geometry is in layout coordinates; screenshot pixel
                // (0, 0) is the frame origin. Refuse rather than pair this
                // capture with a layout that changed meanwhile.
                anyhow::ensure!(
                    crate::wayland::hyprland::desktop_frame()? == *frame,
                    "the Hyprland monitor layout changed during get_desktop_state; call it again"
                );
                frame.rebase_windows(&mut windows);
            } else if let Some(frame) = &niri_frame {
                // niri window geometry is in layout coordinates; screenshot
                // pixel (0, 0) is the single output's logical origin.
                anyhow::ensure!(
                    crate::wayland::niri::desktop_snapshot()?.frame == *frame,
                    "the niri output layout changed during get_desktop_state; call it again"
                );
                frame.rebase_windows(&mut windows);
            }
            let capture_id = crate::capture_action_frame::publish_desktop(
                &capture_service,
                &capture_args,
                &png,
                (shot_w, shot_h),
                (screen_w, screen_h),
            )?;
            Ok((
                b64,
                shot_w,
                shot_h,
                screen_w,
                screen_h,
                scale_factor,
                written,
                windows,
                capture_id,
                overlay_capture,
            ))
        })
        .await;

        match result {
            Ok(Ok((
                b64_opt,
                shot_w,
                shot_h,
                screen_w,
                screen_h,
                scale_factor,
                written,
                windows,
                capture_id,
                overlay_capture,
            ))) => {
                let frame_scale = if shot_w > 0 {
                    f64::from(screen_w) / f64::from(shot_w)
                } else {
                    1.0
                };
                let mut content = Vec::new();
                let mut structured = json!({
                    "platform": "linux",
                    "display": "primary",
                    "screenshot_width": shot_w,
                    "screenshot_height": shot_h,
                    "screen_width": screen_w,
                    "screen_height": screen_h,
                    "scale_factor": scale_factor,
                    "frame_scale": frame_scale,
                    "screenshot_mime_type": "image/png",
                    "windows": windows.iter().map(window_record_json).collect::<Vec<_>>(),
                    "capture_id": capture_id,
                    "agent_overlay_capture": overlay_capture,
                });
                if (frame_scale - 1.0).abs() > 0.001 {
                    // Capped: the uncapped capture is the action frame.
                    structured["screenshot_original_width"] = json!(screen_w);
                    structured["screenshot_original_height"] = json!(screen_h);
                }
                if let Some(b64) = b64_opt {
                    content.push(cua_driver_core::protocol::Content::image_png(b64));
                }
                let window_lines = desktop_window_lines(&windows, frame_scale);
                let mut frame_note = if (frame_scale - 1.0).abs() > 0.001 {
                    format!(
                        "; x/y for scope:\"desktop\" actions are pixels of THIS screenshot \
                         (mapped ×{frame_scale:.2} back to the screen automatically)"
                    )
                } else {
                    String::new()
                };
                cursor_overlay::capture_exclusion::append_summary_note(
                    &mut frame_note,
                    &overlay_capture,
                );
                if let Some(path) = written {
                    structured["screenshot_file_path"] = json!(path);
                    content.push(cua_driver_core::protocol::Content::text(format!(
                        "✅ Desktop screenshot {shot_w}x{shot_h} written to {path} (screen {screen_w}x{screen_h}{frame_note}){window_lines}"
                    )));
                } else {
                    content.push(cua_driver_core::protocol::Content::text(format!(
                        "✅ Desktop screenshot {shot_w}x{shot_h} (screen {screen_w}x{screen_h}{frame_note}){window_lines}"
                    )));
                }
                ToolResult {
                    content,
                    is_error: None,
                    structured_content: Some(structured),
                    action_record: None,
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Capture error: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

/// Text block naming every visible window with the `pid` / `window_id` pair
/// that every other tool takes, so the model's first action targets a real
/// process instead of guessing.
fn desktop_window_lines(windows: &[crate::x11::WindowInfo], frame_scale: f64) -> String {
    let px = |v: i32| (f64::from(v) / frame_scale).round() as i32;
    let sz = |v: u32| (f64::from(v) / frame_scale).round() as u32;
    if windows.is_empty() {
        return "\nVisible windows: none (use list_windows / launch_app).".to_owned();
    }
    let mut out =
        String::from("\nVisible windows (use these pid + window_id values in every tool call):");
    for w in windows {
        let title = if w.title.is_empty() {
            "(no title)".to_owned()
        } else {
            format!("\"{}\"", w.title)
        };
        out.push_str(&format!(
            "\n- pid={} window_id={} {} {}x{} at ({},{}){}",
            w.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
            w.xid,
            title,
            sz(w.width),
            sz(w.height),
            px(w.x),
            px(w.y),
            if w.app_name.is_empty() {
                String::new()
            } else {
                format!(" app={}", w.app_name)
            }
        ));
    }
    out.push_str("\n→ get_window_state(pid, window_id) lists clickable elements; click/type_text take the same pid (+ window_id for x,y).");
    out
}

#[cfg(test)]
mod background_budget_tests {
    use super::*;

    #[test]
    fn synthetic_pointer_drop_detection_covers_gtk_vcl_and_qt() {
        assert!(maps_indicate_synthetic_pointer_dropped(
            "7f /usr/lib/libgtk-3.so.0\n"
        ));
        assert!(maps_indicate_synthetic_pointer_dropped(
            "7f /usr/lib/libgtk-4.so.1\n"
        ));
        assert!(maps_indicate_synthetic_pointer_dropped(
            "7f /opt/libreoffice/program/libmergedlo.so\n"
        ));
        assert!(maps_indicate_synthetic_pointer_dropped(
            "7f /usr/lib/libvcllo.so\n"
        ));
        assert!(maps_indicate_synthetic_pointer_dropped(
            "7f /usr/lib/libQt5Gui.so.5\n"
        ));
        assert!(!maps_indicate_synthetic_pointer_dropped(
            "7f /usr/lib/libgtk-x11-2.0.so.0\n"
        ));
        assert!(!maps_indicate_synthetic_pointer_dropped(
            "7f /usr/lib/libX11.so.6\n"
        ));
    }

    #[test]
    fn desktop_window_lines_expose_pid_and_window_id() {
        let windows = vec![crate::x11::WindowInfo {
            xid: 0x2e00003,
            pid: Some(4321),
            app_name: "libreoffice".into(),
            title: "Untitled 1 - LibreOffice Calc".into(),
            is_on_screen: true,
            z_index: None,
            x: 0,
            y: 27,
            width: 1920,
            height: 1053,
        }];
        let text = desktop_window_lines(&windows, 1.0);
        assert!(text.contains("pid=4321 window_id=48234499"));
        assert!(text.contains("LibreOffice Calc"));
        assert!(text.contains("app=libreoffice"));
        assert!(text.contains("get_window_state(pid, window_id)"));
        assert!(desktop_window_lines(&[], 1.0).contains("none"));
        // A downsized desktop screenshot reports window rects in ITS pixels.
        let scaled = desktop_window_lines(&windows, 1.5);
        assert!(scaled.contains("1280x702 at (0,18)"), "{scaled}");
    }
}
