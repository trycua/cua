//! Custom cursor shape — rasterised from SVG / ICO / PNG.
//!
//! Used when `--cursor-icon <path>` is passed to the MCP binary, plus the
//! `teardrop()` built-in (`cursor-up` from svgrepo) selectable via
//! `--cursor-shape teardrop`. Always produces a 64×64 RGBA pixel buffer.

use anyhow::{bail, Result};

/// Built-in cursor silhouette selectable via `--cursor-shape <name>` or the
/// MCP `cursor_icon` field. Used when no `--cursor-icon` custom file overrides
/// the choice.
///
/// `Teardrop` is the default; opt back into the procedural arrow with
/// `--cursor-shape arrow` (CLI) or `cursor_icon: "arrow"` (MCP).
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
}

impl BuiltinShape {
    /// Canonical `(name → variant)` table. The single source of truth behind
    /// [`parse`](Self::parse), [`names`](Self::names),
    /// [`names_help`](Self::names_help), the CLI `--cursor-shape` help text, and
    /// every platform's MCP `cursor_icon` tool description. Add a built-in here
    /// and all of them pick it up — nothing else hardcodes the name list.
    const TABLE: &'static [(&'static str, Self)] =
        &[("arrow", Self::Arrow), ("teardrop", Self::Teardrop)];

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
    /// `["arrow", "teardrop"]`.
    pub fn names() -> impl Iterator<Item = &'static str> {
        Self::TABLE.iter().map(|(name, _)| *name)
    }

    /// Human-facing list of built-in names for help / tool-description text,
    /// e.g. `'arrow' | 'teardrop'`. The single string the CLI `--help` and every
    /// MCP `cursor_icon` description render from, so the advertised vocabulary
    /// can never drift from what [`parse`](Self::parse) actually accepts.
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
/// - a built-in name (`arrow` / `teardrop`, case-insensitive) → that built-in
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
    /// Selected by `BuiltinShape::Teardrop`, which is the default silhouette.
    pub fn teardrop() -> &'static Self {
        static CACHE: std::sync::OnceLock<CursorShape> = std::sync::OnceLock::new();
        CACHE.get_or_init(|| {
            Self::load_svg_bytes(TEARDROP_CURSOR_SVG).expect("embedded teardrop SVG should parse")
        })
    }

    fn load_svg(path: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        Self::load_svg_bytes(&data)
    }

    fn load_svg_bytes(data: &[u8]) -> Result<Self> {
        let opts = usvg::Options::default();
        let tree = usvg::Tree::from_data(data, &opts)?;

        let mut pixmap = tiny_skia::Pixmap::new(CURSOR_SIZE, CURSOR_SIZE).ok_or_else(|| {
            anyhow::anyhow!("Failed to create {CURSOR_SIZE}×{CURSOR_SIZE} pixmap")
        })?;

        let sx = CURSOR_SIZE as f32 / tree.size().width();
        let sy = CURSOR_SIZE as f32 / tree.size().height();
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(sx, sy),
            &mut pixmap.as_mut(),
        );

        let raw = pixmap.take();
        let pixels = unpremultiply(raw);
        Ok(Self {
            pixels,
            width: CURSOR_SIZE,
            height: CURSOR_SIZE,
        })
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
    }

    #[test]
    fn builtin_shape_rejects_unknown_names() {
        assert_eq!(BuiltinShape::parse(""), None);
        assert_eq!(BuiltinShape::parse("diamond"), None);
        assert_eq!(BuiltinShape::parse("arrow "), None); // whitespace-significant
        assert_eq!(BuiltinShape::parse("cua_brand"), None); // legacy name no longer recognised
                                                            // The invented MCP names that never existed in the renderer must NOT parse —
                                                            // they were doc-only fiction and are gone now.
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
        assert_eq!(help, "'arrow' | 'teardrop'");
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
        // A non-name, non-existent path is treated as a file and fails to load.
        assert!(resolve_cursor_icon("/no/such/cursor.png").is_err());
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
