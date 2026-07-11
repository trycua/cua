//! Custom cursor shape — rasterised from SVG / ICO / PNG.
//!
//! Used when `--cursor-icon <path>` is passed to the MCP binary, plus the
//! `teardrop()` and `sky()` built-ins selectable via `--cursor-shape <name>`.
//! Always produces an RGBA pixel buffer.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Result};

/// Built-in cursor silhouette selectable via `--cursor-shape <name>` or the
/// MCP `cursor_icon` field. Used when no `--cursor-icon` custom file overrides
/// the choice.
///
/// `Teardrop` is the default; opt into another built-in with
/// `--cursor-shape <name>` (CLI) or `cursor_icon: "<name>"` (MCP).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinShape {
    /// Procedural gradient diamond drawn from vector primitives — the
    /// original cua cursor. Sharp at any backing scale because nothing is
    /// rasterised; `draw_default_arrow` rebuilds the path each frame.
    Arrow,
    /// Embedded `cursor-up` SVG (teardrop with notched bottom). Rasterised
    /// once into a 52 px RGBA buffer and blitted with a runtime transform.
    #[default]
    Teardrop,
    /// Embedded Sky kite cursor. Rasterised at the destination display-pixel
    /// size by `paint_cursor`, with the kite tip as the hotspot.
    Sky,
}

impl BuiltinShape {
    /// Canonical `(name → variant)` table. The single source of truth behind
    /// [`parse`](Self::parse), [`names`](Self::names),
    /// [`names_help`](Self::names_help), the CLI `--cursor-shape` help text, and
    /// every platform's MCP `cursor_icon` tool description. Add a built-in here
    /// and all of them pick it up — nothing else hardcodes the name list.
    const TABLE: &'static [(&'static str, Self)] = &[
        ("arrow", Self::Arrow),
        ("teardrop", Self::Teardrop),
        ("sky", Self::Sky),
    ];

    /// Parse the value of `--cursor-shape` / MCP `cursor_icon`. Case-insensitive.
    /// Returns `None` for unknown names so the caller can warn and fall back to
    /// the default.
    pub fn parse(name: &str) -> Option<Self> {
        let lower = name.to_ascii_lowercase();
        Self::TABLE
            .iter()
            .find(|(n, _)| *n == lower)
            .map(|(_, v)| *v)
    }

    /// The accepted built-in names in declaration order, e.g.
    /// `["arrow", "teardrop", "sky"]`.
    pub fn names() -> impl Iterator<Item = &'static str> {
        Self::TABLE.iter().map(|(name, _)| *name)
    }

    /// Human-facing list of built-in names for help / tool-description text,
    /// e.g. `'arrow' | 'teardrop' | 'sky'`. The single string the CLI `--help`
    /// and every MCP `cursor_icon` description render from, so the advertised
    /// vocabulary can never drift from what [`parse`](Self::parse) actually
    /// accepts.
    pub fn names_help() -> String {
        Self::names()
            .map(|n| format!("'{n}'"))
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

/// What an MCP `cursor_icon` value resolves to. Built-in names drive the
/// overlay's `builtin_shape` (so either built-in is reachable regardless of
/// which is the default); a file path becomes a one-off shape override.
#[derive(Debug, Clone)]
pub enum CursorIconResolution {
    /// A built-in silhouette — apply via `OverlayCommand::SetBuiltinShape`,
    /// which sets `builtin_shape` and clears any custom override.
    Builtin(BuiltinShape),
    /// A custom image loaded from disk — apply via
    /// `OverlayCommand::SetShape(Some(_))`.
    Image(CursorShape),
}

/// Resolve an MCP `cursor_icon` (or CLI) value into a [`CursorIconResolution`].
///
/// - empty string → the configured default built-in ([`BuiltinShape::default`])
/// - a built-in name (`arrow` / `teardrop` / `sky`, case-insensitive) → that built-in
/// - anything else → treated as a file path and loaded (`.svg` / `.png` / `.ico`)
///
/// This is the single resolver shared by the CLI flags and every platform's MCP
/// `set_agent_cursor_motion` handler, so the accepted vocabulary — and which
/// rendering path each value takes — stays identical across all of them.
pub fn resolve_cursor_icon(value: &str) -> Result<CursorIconResolution> {
    if value.is_empty() {
        return Ok(CursorIconResolution::Builtin(BuiltinShape::default()));
    }
    if let Some(builtin) = BuiltinShape::parse(value) {
        return Ok(CursorIconResolution::Builtin(builtin));
    }
    CursorShape::load(value).map(CursorIconResolution::Image)
}

/// Rasterised cursor shape.
#[derive(Debug, Clone)]
pub struct CursorShape {
    /// Premultiplied RGBA pixels, row-major top-to-bottom, 4 bytes per pixel.
    /// This is the representation required by `tiny_skia::PixmapRef`, the sole
    /// consumer of cursor assets during compositing.
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Anchor in source pixels that should land at the event coordinate.
    /// Legacy raster shapes use the centre anchor; Sky overrides this so the
    /// kite tip, not the pixmap centre, is the pointing coordinate.
    pub hotspot_x: f32,
    pub hotspot_y: f32,
    /// Degrees added at paint time to align the raster's intrinsic tip angle
    /// with the renderer's heading convention. Teardrop/custom rasters point
    /// up at rest (+90°); Sky points up-left at rest (+135°).
    pub intrinsic_rotation_degrees: f32,
    /// Whether the raster rotates as the motion heading changes. Sky behaves
    /// like the macOS pointer and keeps its up-left orientation fixed.
    pub rotates_with_heading: bool,
}

/// Runtime cursor footprint in logical screen points.
pub const CURSOR_DISPLAY_POINTS: f32 = 26.0;

/// Source rasterisation size. Sized as 2× the runtime display target
/// (26 px) so the downscale ratio stays a clean 2:1.
/// bilinear handles that without smearing, and on 2× retina displays the
/// pixmap maps 1:1 to physical pixels for perfect crispness. Non-integer
/// ratios (e.g. 64→26 = 0.41×) produce visible downscale blur.
pub const CURSOR_SIZE: u32 = 52;

/// Raster orientation offset for teardrop and user-supplied image assets.
pub const DEFAULT_RASTER_INTRINSIC_ROTATION_DEGREES: f32 = 90.0;

/// The Sky asset is supplied as a 37.17×37.17 SVG. `sky()` keeps a 64×64
/// canonical smoke-test raster, while `sky_for_backing_scale()` rasterises at
/// the actual destination pixel footprint to avoid downscale blur on retina.
pub const SKY_CURSOR_SIZE: u32 = 64;

/// Raster orientation offset for the Sky asset, whose tip points up-left.
pub const SKY_RASTER_INTRINSIC_ROTATION_DEGREES: f32 = 135.0;

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

/// Sky kite cursor. The supplied SVG uses `paint-order="stroke"`; encode the
/// outline as a stroke-only path below a fill-only path so rasterisation does
/// not depend on renderer support for paint-order.
const SKY_CURSOR_SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="37.17" height="37.17" viewBox="0 0 18.59 18.59">
  <g transform="translate(3,3)" opacity="0.8">
    <path d="M0.68 1.83 L3.63 9.78 Q4.67 12.59 5.3 9.66 L5.44 9.01 Q6.08 6.08 9.01 5.44 L9.66 5.3 Q12.59 4.67 9.78 3.63 L1.83 0.68 Q0 0 0.68 1.83 Z" fill="none" stroke="#FFFFFF" stroke-width="1.7" stroke-linejoin="round"/>
    <path d="M0.68 1.83 L3.63 9.78 Q4.67 12.59 5.3 9.66 L5.44 9.01 Q6.08 6.08 9.01 5.44 L9.66 5.3 Q12.59 4.67 9.78 3.63 L1.83 0.68 Q0 0 0.68 1.83 Z" fill="#808080"/>
  </g>
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
    /// Selected by `BuiltinShape::Teardrop`, which is the default silhouette.
    pub fn teardrop() -> &'static Self {
        static CACHE: OnceLock<CursorShape> = OnceLock::new();
        CACHE.get_or_init(|| {
            Self::load_svg_bytes(TEARDROP_CURSOR_SVG).expect("embedded teardrop SVG should parse")
        })
    }

    /// The built-in Sky kite cursor. Its SVG viewBox places the click tip at
    /// `(3, 3)`, so the cached shape stores that scaled source-pixel hotspot.
    pub fn sky() -> &'static Self {
        static CACHE: OnceLock<CursorShape> = OnceLock::new();
        CACHE.get_or_init(|| Self::load_sky_at_size(SKY_CURSOR_SIZE))
    }

    /// Rasterise Sky at the display-pixel footprint for this backing scale.
    /// At rest the Sky raster is not rotated, so a 52×52 raster on a 2×
    /// display can be blitted 1:1 instead of resampled from the canonical
    /// 64×64 smoke-test raster.
    pub fn sky_for_backing_scale(backing_scale: f32) -> Self {
        static CACHE: OnceLock<Mutex<HashMap<u32, CursorShape>>> = OnceLock::new();
        let size = sky_size_for_backing_scale(backing_scale);
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = cache.lock().unwrap();
        if let Some(shape) = guard.get(&size) {
            return shape.clone();
        }
        let shape = Self::load_sky_at_size(size);
        guard.insert(size, shape.clone());
        shape
    }

    /// True when the shape still uses the legacy centre anchor and therefore
    /// needs the 16 pt click-position offset used by arrow/teardrop.
    pub fn has_center_hotspot(&self) -> bool {
        const EPS: f32 = 0.01;
        (self.hotspot_x - self.width as f32 / 2.0).abs() < EPS
            && (self.hotspot_y - self.height as f32 / 2.0).abs() < EPS
    }

    fn load_svg(path: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        Self::load_svg_bytes(&data)
    }

    fn load_svg_bytes(data: &[u8]) -> Result<Self> {
        Self::load_svg_bytes_at(data, CURSOR_SIZE)
    }

    fn load_svg_bytes_at(data: &[u8], size: u32) -> Result<Self> {
        let opts = usvg::Options::default();
        let tree = usvg::Tree::from_data(data, &opts)?;

        let mut pixmap = tiny_skia::Pixmap::new(size, size)
            .ok_or_else(|| anyhow::anyhow!("Failed to create {size}×{size} pixmap"))?;

        let sx = size as f32 / tree.size().width();
        let sy = size as f32 / tree.size().height();
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(sx, sy),
            &mut pixmap.as_mut(),
        );

        // resvg renders directly into tiny-skia's premultiplied RGBA format.
        // Preserve it verbatim so the later PixmapRef does not interpret
        // straight-alpha edge colors as over-bright premultiplied channels.
        let pixels = pixmap.take();
        Ok(Self {
            pixels,
            width: size,
            height: size,
            hotspot_x: size as f32 / 2.0,
            hotspot_y: size as f32 / 2.0,
            intrinsic_rotation_degrees: DEFAULT_RASTER_INTRINSIC_ROTATION_DEGREES,
            rotates_with_heading: true,
        })
    }

    fn load_sky_at_size(size: u32) -> Self {
        let mut shape =
            Self::load_svg_bytes_at(SKY_CURSOR_SVG, size).expect("embedded Sky SVG should parse");
        let scale = size as f32 / 18.59;
        shape.hotspot_x = 3.0 * scale;
        shape.hotspot_y = 3.0 * scale;
        shape.intrinsic_rotation_degrees = SKY_RASTER_INTRINSIC_ROTATION_DEGREES;
        shape.rotates_with_heading = false;
        shape
    }

    fn load_raster(path: &str) -> Result<Self> {
        let img = image::open(path)?.into_rgba8();
        Ok(Self {
            pixels: resize_raster_premultiplied(img, CURSOR_SIZE, CURSOR_SIZE),
            width: CURSOR_SIZE,
            height: CURSOR_SIZE,
            hotspot_x: CURSOR_SIZE as f32 / 2.0,
            hotspot_y: CURSOR_SIZE as f32 / 2.0,
            intrinsic_rotation_degrees: DEFAULT_RASTER_INTRINSIC_ROTATION_DEGREES,
            rotates_with_heading: true,
        })
    }
}

/// Premultiply before filtering so hidden RGB in transparent source pixels
/// cannot bleed into antialiased cursor edges during resize.
fn resize_raster_premultiplied(img: image::RgbaImage, width: u32, height: u32) -> Vec<u8> {
    let (source_width, source_height) = img.dimensions();
    let premultiplied = image::RgbaImage::from_raw(
        source_width,
        source_height,
        premultiply_rgba(img.into_raw()),
    )
    .expect("premultiplication preserves the raster buffer size");
    image::imageops::resize(
        &premultiplied,
        width,
        height,
        image::imageops::FilterType::Lanczos3,
    )
    .into_raw()
}

/// Destination pixel footprint for Sky at a backing scale.
pub fn sky_size_for_backing_scale(backing_scale: f32) -> u32 {
    (CURSOR_DISPLAY_POINTS * backing_scale.max(1.0))
        .round()
        .max(1.0) as u32
}

/// Convert straight RGBA, as returned by the `image` crate, into the
/// premultiplied representation required by tiny-skia.
fn premultiply_rgba(mut data: Vec<u8>) -> Vec<u8> {
    for px in data.chunks_exact_mut(4) {
        let a = px[3];
        if a < 255 {
            for channel in &mut px[..3] {
                *channel = ((*channel as u16 * a as u16 + 127) / 255) as u8;
            }
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
        assert_eq!(
            BuiltinShape::parse("teardrop"),
            Some(BuiltinShape::Teardrop)
        );
        assert_eq!(
            BuiltinShape::parse("TEARDROP"),
            Some(BuiltinShape::Teardrop)
        );
        assert_eq!(
            BuiltinShape::parse("Teardrop"),
            Some(BuiltinShape::Teardrop)
        );
        assert_eq!(BuiltinShape::parse("sky"), Some(BuiltinShape::Sky));
        assert_eq!(BuiltinShape::parse("SKY"), Some(BuiltinShape::Sky));
        assert_eq!(BuiltinShape::parse("Sky"), Some(BuiltinShape::Sky));
    }

    #[test]
    fn builtin_shape_rejects_unknown_names() {
        assert_eq!(BuiltinShape::parse(""), None);
        assert_eq!(BuiltinShape::parse("diamond"), None);
        assert_eq!(BuiltinShape::parse("arrow "), None); // whitespace-significant
        assert_eq!(BuiltinShape::parse("cua_brand"), None); // legacy name no longer recognised
        // The invented MCP names that never existed in the renderer must not parse.
        // They were documentation-only fiction and are gone now.
        assert_eq!(BuiltinShape::parse("crosshair"), None);
        assert_eq!(BuiltinShape::parse("hand"), None);
        assert_eq!(BuiltinShape::parse("dot"), None);
    }

    #[test]
    fn names_help_lists_every_table_entry() {
        // Single source of truth: help text is derived from TABLE, so it always
        // matches what `parse` accepts.
        let help = BuiltinShape::names_help();
        for name in BuiltinShape::names() {
            assert!(help.contains(name), "names_help() missing {name}: {help}");
            assert!(
                BuiltinShape::parse(name).is_some(),
                "{name} listed but unparseable"
            );
        }
        assert_eq!(help, "'arrow' | 'teardrop' | 'sky'");
    }

    #[test]
    fn resolve_cursor_icon_matches_builtins_and_revert() {
        use CursorIconResolution::*;
        // Empty reverts to the configured default built-in (now teardrop).
        assert!(matches!(
            resolve_cursor_icon("").unwrap(),
            Builtin(BuiltinShape::Teardrop)
        ));
        // Built-in names resolve to that built-in, case-insensitively —
        // crucially `arrow` stays reachable even though teardrop is the default.
        assert!(matches!(
            resolve_cursor_icon("arrow").unwrap(),
            Builtin(BuiltinShape::Arrow)
        ));
        assert!(matches!(
            resolve_cursor_icon("TEARDROP").unwrap(),
            Builtin(BuiltinShape::Teardrop)
        ));
        assert!(matches!(
            resolve_cursor_icon("sky").unwrap(),
            Builtin(BuiltinShape::Sky)
        ));
        // A non-name, non-existent path is treated as a file and fails to load.
        assert!(resolve_cursor_icon("/no/such/cursor.png").is_err());
    }

    #[test]
    fn sky_raster_has_expected_size_alpha_outline_fill_and_hotspot() {
        let sky = CursorShape::sky();
        assert_eq!((sky.width, sky.height), (SKY_CURSOR_SIZE, SKY_CURSOR_SIZE));
        assert_eq!(
            sky.pixels.len(),
            (SKY_CURSOR_SIZE * SKY_CURSOR_SIZE * 4) as usize
        );

        let mut alpha_pixels = 0usize;
        let mut partial_alpha_pixels = 0usize;
        let mut near_white_pixels = 0usize;
        let mut mid_gray_pixels = 0usize;
        for px in sky.pixels.chunks_exact(4) {
            let [r, g, b, a]: [u8; 4] = px.try_into().unwrap();
            if a > 0 {
                alpha_pixels += 1;
            }
            if a > 0 && a < 255 {
                partial_alpha_pixels += 1;
            }
            assert!(
                r <= a && g <= a && b <= a,
                "cursor assets must stay premultiplied, got rgba({r},{g},{b},{a})"
            );
            if a > 0 {
                let straight =
                    |channel: u8| ((channel as u16 * 255 + a as u16 / 2) / a as u16).min(255) as u8;
                let (sr, sg, sb) = (straight(r), straight(g), straight(b));
                if sr >= 240 && sg >= 240 && sb >= 240 {
                    near_white_pixels += 1;
                }
                if (112..=144).contains(&sr)
                    && (112..=144).contains(&sg)
                    && (112..=144).contains(&sb)
                {
                    mid_gray_pixels += 1;
                }
            }
        }

        assert!(
            alpha_pixels > 0,
            "Sky raster should contain non-transparent pixels"
        );
        assert!(
            partial_alpha_pixels > 0,
            "Sky raster should exercise antialiased partial-alpha edges"
        );
        assert!(
            near_white_pixels > 0,
            "Sky raster should contain near-white outline pixels"
        );
        assert!(
            mid_gray_pixels > 0,
            "Sky raster should contain mid-gray fill pixels"
        );

        let expected_hotspot = 3.0 * SKY_CURSOR_SIZE as f32 / 18.59;
        assert!((sky.hotspot_x - expected_hotspot).abs() < 0.01);
        assert!((sky.hotspot_y - expected_hotspot).abs() < 0.01);
        assert!(!sky.has_center_hotspot());
        assert_eq!(
            sky.intrinsic_rotation_degrees,
            SKY_RASTER_INTRINSIC_ROTATION_DEGREES
        );
        assert!(
            !sky.rotates_with_heading,
            "Sky should keep a fixed up-left orientation"
        );
        assert!(
            CursorShape::teardrop().rotates_with_heading,
            "teardrop should preserve heading-following behavior"
        );
    }

    #[test]
    fn sky_backing_scale_rasters_match_display_pixel_size() {
        assert_eq!(sky_size_for_backing_scale(1.0), 26);
        assert_eq!(sky_size_for_backing_scale(2.0), 52);
        assert_eq!(sky_size_for_backing_scale(3.0), 78);

        let sky_1x = CursorShape::sky_for_backing_scale(1.0);
        let sky_2x = CursorShape::sky_for_backing_scale(2.0);
        let sky_3x = CursorShape::sky_for_backing_scale(3.0);

        assert_eq!((sky_1x.width, sky_1x.height), (26, 26));
        assert_eq!((sky_2x.width, sky_2x.height), (52, 52));
        assert_eq!((sky_3x.width, sky_3x.height), (78, 78));
        assert_eq!(
            sky_2x.intrinsic_rotation_degrees,
            SKY_RASTER_INTRINSIC_ROTATION_DEGREES
        );
        assert!(!sky_2x.has_center_hotspot());
        assert!(!sky_2x.rotates_with_heading);
    }

    #[test]
    fn straight_raster_pixels_are_premultiplied_exactly_once() {
        let pixels = premultiply_rgba(vec![
            255, 128, 64, 128, // translucent color
            250, 200, 150, 0, // fully transparent color must become transparent black
            7, 8, 9, 255, // opaque color is unchanged
        ]);
        assert_eq!(pixels, vec![128, 64, 32, 128, 0, 0, 0, 0, 7, 8, 9, 255]);
    }

    #[test]
    fn raster_resize_filters_premultiplied_pixels_without_hidden_rgb_bleed() {
        let source = image::RgbaImage::from_raw(
            2,
            1,
            vec![
                255, 0, 0, 0, // hidden red must not bleed from transparent input
                0, 0, 255, 255,
            ],
        )
        .unwrap();
        let resized = resize_raster_premultiplied(source, 8, 1);
        for pixel in resized.chunks_exact(4) {
            assert_eq!(pixel[0], 0, "transparent source RGB leaked into the edge");
            assert!(pixel[1] <= pixel[3] && pixel[2] <= pixel[3]);
        }
    }

    /// `Teardrop` is the default silhouette. If this assertion changes, the
    /// `personalize-cursor.mdx` doc and the CLI `--cursor-shape` help must
    /// change to match — both tell users which built-in they get when no flag
    /// is set.
    #[test]
    fn default_builtin_shape_is_teardrop() {
        assert_eq!(BuiltinShape::default(), BuiltinShape::Teardrop);
    }
}
