// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Shared image-manipulation utilities for the `zoom` tool.
//!
//! All three platform crates depend on cursor-overlay, so this is the natural
//! home for a pure-Rust crop + JPEG-encode helper.

use image::{imageops::FilterType, DynamicImage, GenericImageView};

/// Result of a crop-and-encode operation; carries coordinate metadata so callers
/// can translate zoom-image pixel coordinates back to full-window pixel coordinates.
pub struct CropResult {
    pub jpeg_bytes: Vec<u8>,
    /// Output image width in pixels.
    pub out_w: u32,
    /// Output image height in pixels.
    pub out_h: u32,
    /// X of the padded crop origin in full-window pixel space.
    pub origin_x: f64,
    /// Y of the padded crop origin in full-window pixel space.
    pub origin_y: f64,
    /// Inverse of the resize scale: `cw / out_w`.
    /// Multiply a zoom-image X coordinate by this to get the full-window offset from origin.
    /// Equal to 1.0 when no resize was applied, above 1.0 when the crop was
    /// shrunk to fit `max_width`, below 1.0 when a narrow crop was enlarged.
    pub scale_inv: f64,
}

impl CropResult {
    /// Convert a zoom-image coordinate `(px, py)` back to full-window pixel coordinates.
    pub fn zoom_to_window(&self, px: f64, py: f64) -> (f64, f64) {
        (
            self.origin_x + px * self.scale_inv,
            self.origin_y + py * self.scale_inv,
        )
    }
}

/// Largest enlargement applied to a narrow crop.
pub const MAX_UPSCALE: f64 = 4.0;

/// Decode a PNG, crop to the given region with 20% padding on all sides,
/// optionally scale down to `max_width`, and re-encode as JPEG.
pub fn crop_png_to_jpeg(
    png_bytes: &[u8],
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    max_width: u32,
) -> anyhow::Result<CropResult> {
    let img = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)?;
    let (img_w, img_h) = img.dimensions();

    // Region dimensions (at least 1px to avoid divide-by-zero).
    let rw = (x2 - x1).max(1.0);
    let rh = (y2 - y1).max(1.0);

    // 20% outward padding on each side.
    let pad_x = rw * 0.20;
    let pad_y = rh * 0.20;

    let cx1 = ((x1 - pad_x).max(0.0) as u32).min(img_w);
    let cy1 = ((y1 - pad_y).max(0.0) as u32).min(img_h);
    let cx2 = ((x2 + pad_x).min(img_w as f64) as u32).min(img_w);
    let cy2 = ((y2 + pad_y).min(img_h as f64) as u32).min(img_h);

    let cw = cx2.saturating_sub(cx1).max(1);
    let ch = cy2.saturating_sub(cy1).max(1);

    let cropped = img.crop_imm(cx1, cy1, cw, ch);

    // Scale down if the crop is wider than max_width. Scale a narrow crop up
    // (at most MAX_UPSCALE times) so a one-letter flag in a small region is
    // legible instead of a few pixels wide.
    let target_w = if cw > max_width {
        max_width
    } else {
        ((cw as f64 * MAX_UPSCALE).round() as u32)
            .min(max_width)
            .max(cw)
    };
    let out: DynamicImage = if target_w != cw {
        let scale = target_w as f64 / cw as f64;
        let new_h = ((ch as f64) * scale).round().max(1.0) as u32;
        cropped.resize_exact(target_w, new_h, FilterType::Lanczos3)
    } else {
        cropped
    };

    let (out_w, out_h) = out.dimensions();
    let scale_inv = cw as f64 / out_w as f64; // 1.0 if no resize

    // Encode as JPEG.
    let mut buf = Vec::new();
    out.write_to(
        &mut std::io::Cursor::new(&mut buf),
        image::ImageFormat::Jpeg,
    )?;

    Ok(CropResult {
        jpeg_bytes: buf,
        out_w,
        out_h,
        origin_x: cx1 as f64,
        origin_y: cy1 as f64,
        scale_inv,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::new_rgb8(w, h);
        let mut buf = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn a_wide_region_is_shrunk_and_reports_the_shrink() {
        let crop = crop_png_to_jpeg(&png(1600, 800), 100.0, 100.0, 900.0, 300.0, 500).unwrap();
        assert_eq!(crop.out_w, 500);
        assert!(crop.scale_inv > 2.0, "{}", crop.scale_inv);
    }

    #[test]
    fn a_narrow_region_is_enlarged_up_to_the_cap_and_maps_back() {
        let crop = crop_png_to_jpeg(&png(800, 600), 300.0, 200.0, 400.0, 240.0, 500).unwrap();
        // 100 px + 20% padding each side = 140 px, enlarged 3.57x to the 500 px cap.
        assert_eq!(crop.out_w, 500);
        assert!(crop.scale_inv < 1.0);
        let (x, y) = crop.zoom_to_window(250.0, 100.0);
        assert!((x - (280.0 + 250.0 * crop.scale_inv)).abs() < 1e-6);
        assert!(y > 200.0);
    }

    #[test]
    fn a_tiny_region_is_enlarged_at_most_four_times() {
        let crop = crop_png_to_jpeg(&png(800, 600), 300.0, 200.0, 330.0, 220.0, 500).unwrap();
        // 30 px + padding = 42 px, capped at 4x = 168 px.
        assert!(crop.out_w <= (42.0 * MAX_UPSCALE).round() as u32 + 1);
        assert!(crop.out_w > 42);
    }
}
