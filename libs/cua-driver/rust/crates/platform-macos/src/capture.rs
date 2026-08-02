//! Window / display screenshot capture for macOS.
//!
//! ## Window capture (primary: ScreenCaptureKit)
//!
//! Single-frame window capture prefers native ScreenCaptureKit via
//! `SCScreenshotManager::capture_image` with a desktop-independent window
//! filter, then encodes PNG in memory. No subprocess, temp file, or base64
//! on the native success path.
//!
//! Sources:
//! - https://developer.apple.com/documentation/screencapturekit/scscreenshotmanager
//! - https://developer.apple.com/documentation/screencapturekit/sccontentfilter/init(desktopindependentwindow:)
//! - https://docs.rs/screencapturekit/6.0.1/screencapturekit/
//!
//! Compatibility fallback: `screencapture -l <windowID> -x -o <file>` when
//! the native path errors or returns empty bytes. This slice does not yet
//! keep a persistent SCStream or frame cache.
//!
//! ## Display capture
//!
//! `screencapture -x <file>` still captures the full main display (unchanged
//! in this slice).

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use std::process::Command;

struct SecureCapturePath {
    directory: std::path::PathBuf,
    file: std::path::PathBuf,
}

impl SecureCapturePath {
    fn new(file_name: &str) -> anyhow::Result<Self> {
        use std::os::unix::fs::DirBuilderExt;

        let directory = std::env::temp_dir().join(format!(
            "cua-driver-rs-capture-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let file = directory.join(file_name);
        Ok(Self { directory, file })
    }
}

impl Drop for SecureCapturePath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_dir(&self.directory);
    }
}

/// Prefer `native`; on native error or empty bytes, call `fallback`.
///
/// On non-empty native success, returns those bytes without invoking
/// fallback. When both paths fail, the error message preserves both
/// contexts. Closures are not required to be `Send`/`Sync`.
fn capture_window_with_backends<N, F>(
    window_id: u32,
    native: N,
    fallback: F,
) -> anyhow::Result<Vec<u8>>
where
    N: FnOnce(u32) -> anyhow::Result<Vec<u8>>,
    F: FnOnce(u32) -> anyhow::Result<Vec<u8>>,
{
    match native(window_id) {
        Ok(bytes) if !bytes.is_empty() => Ok(bytes),
        Ok(_) => match fallback(window_id) {
            Ok(bytes) => Ok(bytes),
            Err(fallback_err) => Err(anyhow::anyhow!(
                "window {window_id} capture failed: native produced empty bytes; \
                 shell fallback: {fallback_err:#}"
            )),
        },
        Err(native_err) => match fallback(window_id) {
            Ok(bytes) => Ok(bytes),
            Err(fallback_err) => Err(anyhow::anyhow!(
                "window {window_id} capture failed: native: {native_err:#}; \
                 shell fallback: {fallback_err:#}"
            )),
        },
    }
}

/// Shell compatibility path: `screencapture -l <id> -x -o <tmp.png>`.
fn screenshot_window_bytes_shell(window_id: u32) -> anyhow::Result<Vec<u8>> {
    let capture = SecureCapturePath::new("window.png")?;
    let tmp_path = capture.file.to_string_lossy().into_owned();

    let output = Command::new("screencapture")
        .args([
            "-l",
            &window_id.to_string(),
            "-x", // no sound
            "-o", // no shadow
            &tmp_path,
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

    let bytes = std::fs::read(&capture.file)?;

    if bytes.is_empty() {
        anyhow::bail!("screencapture produced empty output for window {window_id}");
    }
    Ok(bytes)
}

/// Hard ceiling on capture dimensions to avoid unbounded allocations.
const MAX_CAPTURE_DIM: u32 = 16384;

fn content_rect_usable(rect: screencapturekit::cg::CGRect) -> bool {
    let w = rect.size.width;
    let h = rect.size.height;
    w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0
}

/// Round a positive finite pixel extent into `1..=MAX_CAPTURE_DIM` as `u32`.
fn rounded_pixel_dim(value: f64, label: &str) -> anyhow::Result<u32> {
    if !value.is_finite() || value <= 0.0 {
        anyhow::bail!("invalid capture {label}: {value}");
    }
    let rounded = value.round();
    if !(rounded.is_finite()) || rounded < 1.0 || rounded > f64::from(MAX_CAPTURE_DIM) {
        anyhow::bail!("capture {label} {rounded} out of allowed range 1..={MAX_CAPTURE_DIM}");
    }
    // After the range check the value fits in u32.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let dim = rounded as u32;
    if dim == 0 || dim > MAX_CAPTURE_DIM {
        anyhow::bail!("capture {label} {dim} out of allowed range 1..={MAX_CAPTURE_DIM}");
    }
    Ok(dim)
}

fn checked_image_dim(value: usize, label: &str) -> anyhow::Result<u32> {
    if value == 0 || value > MAX_CAPTURE_DIM as usize {
        anyhow::bail!("{label} {value} out of allowed range 1..={MAX_CAPTURE_DIM}");
    }
    u32::try_from(value).map_err(|_| anyhow::anyhow!("{label} {value} does not fit u32"))
}

/// Native ScreenCaptureKit single-frame window capture (in-process PNG).
///
/// Uses a desktop-independent window filter and `SCScreenshotManager` —
/// see module docs for Apple + crate source URLs. No subprocess/temp/base64.
fn screenshot_window_bytes_sck(window_id: u32) -> anyhow::Result<Vec<u8>> {
    use screencapturekit::prelude::{SCContentFilter, SCShareableContent, SCStreamConfiguration};
    use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};

    let content = SCShareableContent::get()
        .map_err(|e| anyhow::anyhow!("SCShareableContent::get failed: {e}"))?;

    let window = content
        .windows()
        .into_iter()
        .find(|w| w.window_id() == window_id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "ScreenCaptureKit: window id {window_id} not found in shareable content \
                 (closed, not capturable, or no longer listed)"
            )
        })?;

    // Desktop-independent window filter (captures only the specified window):
    // https://developer.apple.com/documentation/screencapturekit/sccontentfilter/init(desktopindependentwindow:)
    let filter = SCContentFilter::create().with_window(&window).build();

    // Pixel output size = content_rect * point_pixel_scale (macOS 14+ filter info).
    // https://docs.rs/screencapturekit/6.0.1/screencapturekit/
    let scale = f64::from(filter.point_pixel_scale());
    let (width_pts, height_pts) = {
        let rect = filter.content_rect();
        if content_rect_usable(rect) {
            (rect.size.width, rect.size.height)
        } else {
            let frame = window.frame();
            (frame.size.width, frame.size.height)
        }
    };

    let out_w = rounded_pixel_dim(width_pts * scale, "width")?;
    let out_h = rounded_pixel_dim(height_pts * scale, "height")?;

    let config = SCStreamConfiguration::new()
        .with_width(out_w)
        .with_height(out_h);

    // Single-frame capture (no persistent stream/cache in this slice):
    // https://developer.apple.com/documentation/screencapturekit/scscreenshotmanager
    let image = SCScreenshotManager::capture_image(&filter, &config).map_err(|e| {
        anyhow::anyhow!("SCScreenshotManager::capture_image failed for window {window_id}: {e}")
    })?;

    let w = checked_image_dim(image.width(), "CGImage width")?;
    let h = checked_image_dim(image.height(), "CGImage height")?;

    let rgba = image
        .rgba_data()
        .map_err(|e| anyhow::anyhow!("CGImage::rgba_data failed for window {window_id}: {e}"))?;

    let expected_len = (w as u64)
        .checked_mul(h as u64)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("RGBA byte length overflow for {w}x{h}"))?;
    if rgba.len() as u64 != expected_len {
        anyhow::bail!(
            "CGImage RGBA length {} != {w}*{h}*4 ({expected_len}) for window {window_id}",
            rgba.len()
        );
    }

    cua_driver_core::image_utils::encode_rgba_to_png(&rgba, w, h)
}

/// Capture a window by its `window_id` (CGWindowID).
/// Returns raw PNG bytes or an error.
///
/// Tries ScreenCaptureKit first; falls back to the `screencapture` CLI on
/// native error or empty output.
pub fn screenshot_window_bytes(window_id: u32) -> anyhow::Result<Vec<u8>> {
    capture_window_with_backends(
        window_id,
        screenshot_window_bytes_sck,
        screenshot_window_bytes_shell,
    )
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
    let capture = SecureCapturePath::new("display.png")?;
    let tmp_path = capture.file.to_string_lossy().into_owned();

    let output = Command::new("screencapture")
        .args(["-x", &tmp_path])
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

    let bytes = std::fs::read(&capture.file)?;

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
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn native_window_capture_short_circuits_shell_fallback() {
        let png = cua_driver_core::image_utils::encode_rgba_to_png(&[0, 0, 0, 255], 1, 1)
            .expect("encode 1x1 PNG");
        assert!(!png.is_empty(), "PNG bytes must be non-empty");

        let native_calls = Rc::new(Cell::new(0u32));
        let fallback_calls = Rc::new(Cell::new(0u32));
        let native_window_id = Rc::new(Cell::new(None::<u32>));

        let native_calls_n = Rc::clone(&native_calls);
        let native_window_id_n = Rc::clone(&native_window_id);
        let png_n = png.clone();
        let fallback_calls_f = Rc::clone(&fallback_calls);

        let got = capture_window_with_backends(
            42,
            move |window_id| {
                native_calls_n.set(native_calls_n.get() + 1);
                native_window_id_n.set(Some(window_id));
                Ok(png_n)
            },
            move |_window_id| {
                fallback_calls_f.set(fallback_calls_f.get() + 1);
                anyhow::bail!("shell fallback must not run when native succeeds");
            },
        )
        .expect("native capture should succeed");

        assert_eq!(native_calls.get(), 1, "native backend called once");
        assert_eq!(
            native_window_id.get(),
            Some(42),
            "native backend receives window id 42"
        );
        assert_eq!(fallback_calls.get(), 0, "shell fallback must not run");
        assert_eq!(got, png, "helper returns native PNG bytes verbatim");
    }
}
