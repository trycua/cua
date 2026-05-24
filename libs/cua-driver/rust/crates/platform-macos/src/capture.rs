//! Window / display screenshot using the macOS `screencapture` CLI tool.
//!
//! `screencapture -l <windowID> -x -o <file>` captures a single window by
//! CGWindowID to a PNG without any screen-recording permission dialog.
//!
//! `screencapture -x <file>` captures the full main display.
//!
//! For production use, the ImageIO/CGWindowListCreateImageFromArray path
//! would give lower overhead (no subprocess + temp file), but the subprocess
//! approach is simpler to implement correctly and is reliable across OS versions.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use std::process::Command;

/// Capture a window by its `window_id` (CGWindowID).
/// Returns raw PNG bytes or an error.
pub fn screenshot_window_bytes(window_id: u32) -> anyhow::Result<Vec<u8>> {
    let tmp_path = format!("/tmp/cua-driver-rs-capture-{}.png", window_id);

    let status = Command::new("screencapture")
        .args([
            "-l", &window_id.to_string(),
            "-x",  // no sound
            "-o",  // no shadow
            &tmp_path,
        ])
        .status()?;

    if !status.success() {
        anyhow::bail!("screencapture failed for window {window_id}");
    }

    let bytes = std::fs::read(&tmp_path)?;
    let _ = std::fs::remove_file(&tmp_path);

    if bytes.is_empty() {
        anyhow::bail!("screencapture produced empty output for window {window_id}");
    }
    Ok(bytes)
}

/// Capture a window by its `window_id` (CGWindowID).
/// Returns (base64-encoded PNG, width, height) or an error.
pub fn screenshot_window(window_id: u32) -> anyhow::Result<(String, u32, u32)> {
    let bytes = screenshot_window_bytes(window_id)?;
    let (w, h) = png_dimensions(&bytes)?;
    let b64 = BASE64.encode(&bytes);
    Ok((b64, w, h))
}

/// Capture the full main display.
/// Returns raw PNG bytes or an error.
pub fn screenshot_display_bytes() -> anyhow::Result<Vec<u8>> {
    // Use a pid-unique path so concurrent cua-driver processes don't step on each other.
    let tmp_path = format!("/tmp/cua-driver-rs-display-{}.png", std::process::id());

    let status = Command::new("screencapture")
        .args(["-x", &*tmp_path])
        .status()?;

    if !status.success() {
        anyhow::bail!("screencapture failed for main display");
    }

    let bytes = std::fs::read(&tmp_path)?;
    let _ = std::fs::remove_file(&tmp_path);

    if bytes.is_empty() {
        anyhow::bail!("screencapture produced empty output for main display");
    }
    Ok(bytes)
}

/// Capture the main display and return (base64-encoded PNG, width, height).
pub fn screenshot_display() -> anyhow::Result<(String, u32, u32)> {
    let bytes = screenshot_display_bytes()?;
    let (w, h) = png_dimensions(&bytes)?;
    let b64 = BASE64.encode(&bytes);
    Ok((b64, w, h))
}

// PNG/JPEG/resize/crosshair helpers — re-exports of the shared
// `mcp_server::image_utils` module. The previous file-local copies were
// near-identical to the Windows and Linux versions; the dedup-audit
// (2026-05) moved them all to one place. See
// `CUA_DRIVER_RS_DEDUP_AUDIT.md` for the audit trail.

/// Convert raw PNG bytes to JPEG at the given quality (1-95).
pub fn png_bytes_to_jpeg(png_bytes: &[u8], quality: u8) -> anyhow::Result<Vec<u8>> {
    mcp_server::image_utils::png_bytes_to_jpeg(png_bytes, quality)
}

/// Downscale `png_bytes` so neither dimension exceeds `max_dim`.
/// If `max_dim == 0` or the image already fits, returns the original
/// bytes unchanged.
pub fn resize_png_if_needed(png_bytes: &[u8], max_dim: u32) -> anyhow::Result<Vec<u8>> {
    mcp_server::image_utils::resize_png_if_needed(png_bytes, max_dim)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and write to
/// `path`. Used by `click`'s `debug_image_out` param to verify
/// coordinate spaces. The crosshair uses top-left-origin coords
/// matching the click tool's convention.
pub fn write_crosshair_png(
    png_bytes: &[u8],
    cx: f64,
    cy: f64,
    path: &str,
) -> anyhow::Result<()> {
    mcp_server::image_utils::write_crosshair_png(png_bytes, cx, cy, path)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and return the
/// modified PNG bytes. Used by recording's click-marker callback to
/// produce click.png.
pub fn crosshair_png_bytes(png_bytes: &[u8], cx: f64, cy: f64) -> anyhow::Result<Vec<u8>> {
    mcp_server::image_utils::crosshair_png_bytes(png_bytes, cx, cy)
}

/// Parse width and height from a PNG file's IHDR chunk.
pub fn png_dimensions(data: &[u8]) -> anyhow::Result<(u32, u32)> {
    mcp_server::image_utils::png_dimensions(data)
}
