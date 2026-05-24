//! Custom cursor shape — rasterised from SVG / ICO / PNG.
//!
//! Used when `--cursor-icon <path>` is passed to the MCP binary.
//! Always produces a 64×64 RGBA pixel buffer.

use anyhow::{bail, Result};

/// Rasterised cursor shape at 64×64 RGBA.
#[derive(Debug, Clone)]
pub struct CursorShape {
    /// Raw RGBA pixels, row-major top-to-bottom, 4 bytes per pixel.
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub const CURSOR_SIZE: u32 = 64;

impl CursorShape {
    /// Load from `path`.  Supported: `.svg`, `.ico`, `.png`.
    pub fn load(path: &str) -> Result<Self> {
        let lower = path.to_ascii_lowercase();
        if lower.ends_with(".svg") {
            Self::load_svg(path)
        } else if lower.ends_with(".ico") || lower.ends_with(".png") {
            Self::load_raster(path)
        } else {
            bail!("Unsupported cursor-icon format (expected .svg, .ico, or .png): {path}")
        }
    }

    fn load_svg(path: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        let opts = usvg::Options::default();
        let tree = usvg::Tree::from_data(&data, &opts)?;

        let mut pixmap = tiny_skia::Pixmap::new(CURSOR_SIZE, CURSOR_SIZE)
            .ok_or_else(|| anyhow::anyhow!("Failed to create {CURSOR_SIZE}×{CURSOR_SIZE} pixmap"))?;

        let sx = CURSOR_SIZE as f32 / tree.size().width();
        let sy = CURSOR_SIZE as f32 / tree.size().height();
        resvg::render(&tree, tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());

        // tiny-skia gives pre-multiplied RGBA; un-premultiply for straight-alpha consumers.
        let raw = pixmap.take();
        let pixels = unpremultiply(raw);
        Ok(Self { pixels, width: CURSOR_SIZE, height: CURSOR_SIZE })
    }

    fn load_raster(path: &str) -> Result<Self> {
        let img = image::open(path)?.into_rgba8();
        let resized = image::imageops::resize(
            &img,
            CURSOR_SIZE,
            CURSOR_SIZE,
            image::imageops::FilterType::Lanczos3,
        );
        Ok(Self {
            pixels: resized.into_raw(),
            width: CURSOR_SIZE,
            height: CURSOR_SIZE,
        })
    }
}

/// Convert pre-multiplied RGBA → straight RGBA.
fn unpremultiply(mut data: Vec<u8>) -> Vec<u8> {
    for px in data.chunks_exact_mut(4) {
        let a = px[3];
        if a > 0 && a < 255 {
            let scale = 255.0 / a as f32;
            px[0] = (px[0] as f32 * scale).min(255.0) as u8;
            px[1] = (px[1] as f32 * scale).min(255.0) as u8;
            px[2] = (px[2] as f32 * scale).min(255.0) as u8;
        }
    }
    data
}
