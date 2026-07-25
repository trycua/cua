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
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static WINDOW_CAPTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn window_capture_temp_path(window_id: u32) -> PathBuf {
    let sequence = WINDOW_CAPTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "cua-driver-rs-capture-{}-{window_id}-{sequence}.png",
        std::process::id()
    ))
}

struct WindowCaptureTempFile {
    path: PathBuf,
}

impl WindowCaptureTempFile {
    fn new(window_id: u32) -> Self {
        Self {
            path: window_capture_temp_path(window_id),
        }
    }
}

impl Drop for WindowCaptureTempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Capture a window by its `window_id` (CGWindowID).
/// Returns raw PNG bytes or an error.
pub fn screenshot_window_bytes(window_id: u32) -> anyhow::Result<Vec<u8>> {
    let tmp_file = WindowCaptureTempFile::new(window_id);

    let output = Command::new("screencapture")
        .args([
            "-l",
            &window_id.to_string(),
            "-x", // no sound
            "-o", // no shadow
            tmp_file.path.to_string_lossy().as_ref(),
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.is_empty() {
            anyhow::bail!(
                "screencapture failed for window {window_id} with status {}",
                output.status
            );
        }
        anyhow::bail!(
            "screencapture failed for window {window_id} with status {}: {stderr}",
            output.status
        );
    }

    let bytes = std::fs::read(&tmp_file.path)?;

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

    let output = Command::new("screencapture")
        .args(["-x", &*tmp_path])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.is_empty() {
            anyhow::bail!(
                "screencapture failed for main display with status {}",
                output.status
            );
        }
        anyhow::bail!(
            "screencapture failed for main display with status {}: {stderr}",
            output.status
        );
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
// `cua_driver_core::image_utils` module. The previous file-local copies were
// near-identical to the Windows and Linux versions; the dedup-audit
// (2026-05) moved them all to one place. See
// `CUA_DRIVER_RS_DEDUP_AUDIT.md` for the audit trail.

/// Convert raw PNG bytes to JPEG at the given quality (1-95).
pub fn png_bytes_to_jpeg(png_bytes: &[u8], quality: u8) -> anyhow::Result<Vec<u8>> {
    cua_driver_core::image_utils::png_bytes_to_jpeg(png_bytes, quality)
}

/// Downscale `png_bytes` so neither dimension exceeds `max_dim`.
/// If `max_dim == 0` or the image already fits, returns the original
/// bytes unchanged.
pub fn resize_png_if_needed(png_bytes: &[u8], max_dim: u32) -> anyhow::Result<Vec<u8>> {
    cua_driver_core::image_utils::resize_png_if_needed(png_bytes, max_dim)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and write to
/// `path`. Used by `click`'s `debug_image_out` param to verify
/// coordinate spaces. The crosshair uses top-left-origin coords
/// matching the click tool's convention.
pub fn write_crosshair_png(png_bytes: &[u8], cx: f64, cy: f64, path: &str) -> anyhow::Result<()> {
    cua_driver_core::image_utils::write_crosshair_png(png_bytes, cx, cy, path)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and return the
/// modified PNG bytes. Used by recording's click-marker callback to
/// produce click.png.
pub fn crosshair_png_bytes(png_bytes: &[u8], cx: f64, cy: f64) -> anyhow::Result<Vec<u8>> {
    cua_driver_core::image_utils::crosshair_png_bytes(png_bytes, cx, cy)
}

/// Parse width and height from a PNG file's IHDR chunk.
pub fn png_dimensions(data: &[u8]) -> anyhow::Result<(u32, u32)> {
    cua_driver_core::image_utils::png_dimensions(data)
}

#[cfg(test)]
mod tests {
    use super::{window_capture_temp_path, WindowCaptureTempFile};

    #[test]
    fn window_capture_temp_path_is_unique_per_call() {
        let first = window_capture_temp_path(129913);
        let second = window_capture_temp_path(129913);

        assert_ne!(first, second);
        let first_name = first.file_name().unwrap().to_string_lossy();
        assert!(first_name.contains("129913"));
        assert!(first_name.starts_with("cua-driver-rs-capture-"));
    }

    #[test]
    fn window_capture_temp_file_is_removed_on_drop() {
        let temp_file = WindowCaptureTempFile::new(129913);
        let path = temp_file.path.clone();
        std::fs::write(&path, b"partial capture").unwrap();

        drop(temp_file);

        assert!(!path.exists());
    }
}
