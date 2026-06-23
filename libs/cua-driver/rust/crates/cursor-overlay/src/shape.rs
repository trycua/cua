//! Custom cursor shape — rasterised from SVG / ICO / PNG.
//!
//! Used when `--cursor-icon <path>` is passed to the MCP binary, plus the
//! `teardrop()` built-in (`cursor-up` from svgrepo) selectable via
//! `--cursor-shape teardrop`. Always produces a 64×64 RGBA pixel buffer.

use anyhow::{bail, Result};

/// Built-in cursor silhouette selectable via `--cursor-shape <name>`.
/// Used when no `--cursor-icon` custom file overrides the choice.
///
/// `Arrow` is the default until the teardrop's retina rasterisation is
/// fully sorted; users can opt into the SVG via `--cursor-shape teardrop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinShape {
    /// Procedural gradient diamond drawn from vector primitives — the
    /// original cua cursor. Sharp at any backing scale because nothing is
    /// rasterised; `draw_default_arrow` rebuilds the path each frame.
    Arrow,
    /// Embedded `cursor-up` SVG (teardrop with notched bottom). Rasterised
    /// once into a 52 px RGBA buffer and blitted with a runtime transform.
    Teardrop,
}

impl BuiltinShape {
    /// Parse the value of `--cursor-shape`. Case-insensitive. Returns
    /// `None` for unknown names so the caller can warn and fall back to
    /// the default.
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "arrow" => Some(Self::Arrow),
            "teardrop" => Some(Self::Teardrop),
            _ => None,
        }
    }
}

impl Default for BuiltinShape {
    fn default() -> Self {
        Self::Arrow
    }
}

/// Rasterised cursor shape at 64×64 RGBA.
#[derive(Debug, Clone)]
pub struct CursorShape {
    /// Raw RGBA pixels, row-major top-to-bottom, 4 bytes per pixel.
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Source rasterisation size. Sized as 2× the runtime display target
/// (26 px in `paint_cursor`) so the downscale ratio stays a clean 2:1 —
/// bilinear handles that without smearing, and on 2× retina displays the
/// pixmap maps 1:1 to physical pixels for perfect crispness. Non-integer
/// ratios (e.g. 64→26 = 0.41×) produce visible downscale blur.
pub const CURSOR_SIZE: u32 = 52;

/// User-supplied "cursor-up" silhouette from svgrepo.com — an upward-pointing
/// teardrop with a notched bottom. Original fill was solid black; we substitute
/// the fleet linear gradient (light blue → teal, top-right to bottom-left) so
/// the runtime `gradient_colors` MCP override has a surface to tint, and add a
/// white stroke for visibility on dark backdrops. Embedded verbatim so the
/// daemon ships the built-in teardrop without an external asset.
const TEARDROP_CURSOR_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
<defs>
<linearGradient id="cursorGrad" x1="1" x2="0" y1="0" y2="1">
<stop offset="0" stop-color="#F0FBFF"/>
<stop offset="0.55" stop-color="#66D9FF"/>
<stop offset="1" stop-color="#35C6D8"/>
</linearGradient>
</defs>
<path d="M19.87,19.21l-6-15.92a2,2,0,0,0-3.74,0l-6,15.92a2,2,0,0,0,.65,2.3A2.21,2.21,0,0,0,6.17,22a2.24,2.24,0,0,0,1.23-.37L12,18.57l4.6,3.06a2.22,2.22,0,0,0,2.62-.12A2,2,0,0,0,19.87,19.21Z" fill="url(#cursorGrad)" stroke="#FFFFFF" stroke-width="1.5" stroke-linejoin="round" stroke-linecap="round"/>
</svg>"##;

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

    /// The built-in teardrop cursor — embedded `cursor-up` silhouette
    /// rasterised once via `OnceLock` so callers can ask for it cheaply.
    /// Selected at runtime by `BuiltinShape::Teardrop`; opt-in via
    /// `--cursor-shape teardrop`.
    pub fn teardrop() -> &'static Self {
        static CACHE: std::sync::OnceLock<CursorShape> = std::sync::OnceLock::new();
        CACHE.get_or_init(|| {
            Self::load_svg_bytes(TEARDROP_CURSOR_SVG)
                .expect("embedded teardrop SVG should parse")
        })
    }

    fn load_svg(path: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        Self::load_svg_bytes(&data)
    }

    fn load_svg_bytes(data: &[u8]) -> Result<Self> {
        let opts = usvg::Options::default();
        let tree = usvg::Tree::from_data(data, &opts)?;

        let mut pixmap = tiny_skia::Pixmap::new(CURSOR_SIZE, CURSOR_SIZE)
            .ok_or_else(|| anyhow::anyhow!("Failed to create {CURSOR_SIZE}×{CURSOR_SIZE} pixmap"))?;

        let sx = CURSOR_SIZE as f32 / tree.size().width();
        let sy = CURSOR_SIZE as f32 / tree.size().height();
        resvg::render(&tree, tiny_skia::Transform::from_scale(sx, sy), &mut pixmap.as_mut());

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_shape_parses_known_names_case_insensitive() {
        assert_eq!(BuiltinShape::parse("arrow"), Some(BuiltinShape::Arrow));
        assert_eq!(BuiltinShape::parse("ARROW"), Some(BuiltinShape::Arrow));
        assert_eq!(BuiltinShape::parse("Arrow"), Some(BuiltinShape::Arrow));
        assert_eq!(BuiltinShape::parse("teardrop"), Some(BuiltinShape::Teardrop));
        assert_eq!(BuiltinShape::parse("TEARDROP"), Some(BuiltinShape::Teardrop));
        assert_eq!(BuiltinShape::parse("Teardrop"), Some(BuiltinShape::Teardrop));
    }

    #[test]
    fn builtin_shape_rejects_unknown_names() {
        assert_eq!(BuiltinShape::parse(""), None);
        assert_eq!(BuiltinShape::parse("diamond"), None);
        assert_eq!(BuiltinShape::parse("arrow "), None); // whitespace-significant
        assert_eq!(BuiltinShape::parse("cua_brand"), None); // legacy name no longer recognised
    }

    /// The default is `Arrow` while the teardrop's retina rasterisation
    /// is still being tightened. If this assertion changes, the
    /// `personalize-cursor.mdx` doc must change to match — the doc
    /// explicitly tells users which built-in they get when no flag is set.
    #[test]
    fn default_builtin_shape_is_arrow() {
        assert_eq!(BuiltinShape::default(), BuiltinShape::Arrow);
    }
}
