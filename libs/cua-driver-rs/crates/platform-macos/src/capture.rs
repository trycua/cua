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

/// Convert raw PNG bytes to JPEG at the given quality (1-95).
/// Uses the `cursor_overlay::capture_utils` JPEG encoder.
pub fn png_bytes_to_jpeg(png_bytes: &[u8], quality: u8) -> anyhow::Result<Vec<u8>> {
    use image::ImageDecoder;
    let cursor = std::io::Cursor::new(png_bytes);
    let decoder = image::codecs::png::PngDecoder::new(cursor)?;
    let (w, h) = decoder.dimensions();
    let color = decoder.color_type();
    let mut buf = vec![0u8; decoder.total_bytes() as usize];
    decoder.read_image(&mut buf)?;

    // Ensure RGBA → RGB for JPEG encoding.
    let rgb_buf: Vec<u8> = if color == image::ColorType::Rgba8 {
        buf.chunks_exact(4).flat_map(|px| [px[0], px[1], px[2]]).collect()
    } else {
        buf
    };

    let mut jpeg_bytes = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, quality);
    enc.encode(&rgb_buf, w, h, image::ColorType::Rgb8.into())?;
    Ok(jpeg_bytes)
}

/// Downscale `png_bytes` so neither dimension exceeds `max_dim`.
/// If `max_dim == 0` or the image already fits, returns the original bytes unchanged.
pub fn resize_png_if_needed(png_bytes: &[u8], max_dim: u32) -> anyhow::Result<Vec<u8>> {
    if max_dim == 0 {
        return Ok(png_bytes.to_vec());
    }
    let (w, h) = png_dimensions(png_bytes)?;
    if w <= max_dim && h <= max_dim {
        return Ok(png_bytes.to_vec());
    }
    // Determine scale factor to fit within max_dim x max_dim.
    let scale = (max_dim as f64) / (w.max(h) as f64);
    let new_w = (w as f64 * scale).round() as u32;
    let new_h = (h as f64 * scale).round() as u32;

    use image::ImageDecoder;
    let cursor = std::io::Cursor::new(png_bytes);
    let decoder = image::codecs::png::PngDecoder::new(cursor)?;
    let color = decoder.color_type();
    let mut buf = vec![0u8; decoder.total_bytes() as usize];
    decoder.read_image(&mut buf)?;

    let img = match color {
        image::ColorType::Rgba8 => {
            image::DynamicImage::ImageRgba8(
                image::ImageBuffer::from_raw(w, h, buf)
                    .ok_or_else(|| anyhow::anyhow!("invalid RGBA buffer"))?,
            )
        }
        image::ColorType::Rgb8 => {
            image::DynamicImage::ImageRgb8(
                image::ImageBuffer::from_raw(w, h, buf)
                    .ok_or_else(|| anyhow::anyhow!("invalid RGB buffer"))?,
            )
        }
        _ => anyhow::bail!("unsupported color type for resize: {color:?}"),
    };

    let resized = img.resize(new_w, new_h, image::imageops::FilterType::Lanczos3);
    let mut out = Vec::new();
    resized.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)?;
    Ok(out)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and write to `path`.
/// Used by `click`'s `debug_image_out` param to verify coordinate spaces.
/// The crosshair uses top-left-origin coords matching the click tool's convention.
pub fn write_crosshair_png(
    png_bytes: &[u8],
    cx: f64,
    cy: f64,
    path: &str,
) -> anyhow::Result<()> {
    use image::{ImageDecoder, DynamicImage};

    let cursor = std::io::Cursor::new(png_bytes);
    let decoder = image::codecs::png::PngDecoder::new(cursor)?;
    let (w, h) = decoder.dimensions();
    let color = decoder.color_type();
    let mut buf = vec![0u8; decoder.total_bytes() as usize];
    decoder.read_image(&mut buf)?;

    let mut img: DynamicImage = match color {
        image::ColorType::Rgba8 => DynamicImage::ImageRgba8(
            image::ImageBuffer::from_raw(w, h, buf)
                .ok_or_else(|| anyhow::anyhow!("invalid RGBA buffer"))?,
        ),
        image::ColorType::Rgb8 => DynamicImage::ImageRgb8(
            image::ImageBuffer::from_raw(w, h, buf)
                .ok_or_else(|| anyhow::anyhow!("invalid RGB buffer"))?,
        ).into(),
        _ => anyhow::bail!("unsupported color type for crosshair: {color:?}"),
    };
    // Ensure RGBA for drawing.
    let mut img = img.to_rgba8();

    // Crosshair geometry.
    let ring_r = (w as f64 / 80.0).max(6.0) as i32;
    let arm_len = (w as f64 / 40.0).max(12.0) as i32;
    let line_w  = ((w as f64 / 400.0).max(1.5)) as i32;
    let red     = image::Rgba([255u8, 26, 26, 242]);
    let cx = cx as i32;
    let cy = cy as i32;

    // Draw horizontal + vertical arms.
    for lw in 0..=line_w {
        let off = lw - line_w / 2;
        for dx in -arm_len..=arm_len {
            if let Some(p) = img.get_pixel_mut_checked(
                (cx + dx).clamp(0, w as i32 - 1) as u32,
                (cy + off).clamp(0, h as i32 - 1) as u32,
            ) { *p = red; }
        }
        for dy in -arm_len..=arm_len {
            if let Some(p) = img.get_pixel_mut_checked(
                (cx + off).clamp(0, w as i32 - 1) as u32,
                (cy + dy).clamp(0, h as i32 - 1) as u32,
            ) { *p = red; }
        }
    }

    // Draw ring (stroke circle).
    let steps = (ring_r * 12).max(48) as usize;
    for i in 0..steps {
        let theta = 2.0 * std::f64::consts::PI * i as f64 / steps as f64;
        let rx = (cx as f64 + ring_r as f64 * theta.cos()) as i32;
        let ry = (cy as f64 + ring_r as f64 * theta.sin()) as i32;
        for lw in 0..=line_w {
            let off = lw - line_w / 2;
            for dx in 0..=1 {
                let fx = (rx + off + dx).clamp(0, w as i32 - 1) as u32;
                let fy = (ry + off).clamp(0, h as i32 - 1) as u32;
                *img.get_pixel_mut(fx, fy) = red;
            }
        }
    }

    // Write PNG to path.
    let path = if let Some(rest) = path.strip_prefix('~') {
        format!("{}{}", std::env::var("HOME").unwrap_or_default(), rest)
    } else {
        path.to_owned()
    };
    img.save_with_format(&path, image::ImageFormat::Png)?;
    Ok(())
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and return the modified PNG bytes.
/// Used by recording's click-marker callback to produce click.png.
pub fn crosshair_png_bytes(png_bytes: &[u8], cx: f64, cy: f64) -> anyhow::Result<Vec<u8>> {
    use std::io::Cursor;
    let tmp_path = format!("/tmp/cua-driver-rs-clickmarker-{}.png", std::process::id());
    write_crosshair_png(png_bytes, cx, cy, &tmp_path)?;
    let out = std::fs::read(&tmp_path)?;
    let _ = std::fs::remove_file(&tmp_path);
    Ok(out)
}

/// Parse width and height from a PNG file's IHDR chunk.
pub fn png_dimensions(data: &[u8]) -> anyhow::Result<(u32, u32)> {
    // PNG signature: 8 bytes, IHDR chunk: 4-byte length, "IHDR", 4-byte width, 4-byte height
    if data.len() < 24 {
        anyhow::bail!("PNG data too small");
    }
    // Signature check
    if &data[0..8] != b"\x89PNG\r\n\x1a\n" {
        anyhow::bail!("Not a PNG file");
    }
    let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    Ok((w, h))
}
