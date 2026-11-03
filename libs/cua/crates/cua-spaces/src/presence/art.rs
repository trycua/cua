//! The shared presence cursor art (`assets/presence-cursors.json`): one SVG
//! path per [`CursorShape`] on a 32x32 canvas, filled with the participant's
//! color over a white outline. Every client draws these; the TypeScript copy
//! (`libs/cua/typescript/src/spaces/cursorArt.ts`) is generated from the same
//! JSON with a `--check` drift gate, and Swift reads it through the SDK.

use std::sync::OnceLock;

use super::view::CursorShape;

/// The art JSON, verbatim.
pub const CURSOR_ART_JSON: &str = include_str!("../../assets/presence-cursors.json");

/// One shape's art.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct CursorArt {
    /// The shape.
    pub shape: CursorShape,
    /// SVG path data (absolute `M`, `L`, `C`, `Z` only; nonzero fill).
    pub path_d: String,
    /// Hot spot x on the canvas.
    pub hotspot_x: f64,
    /// Hot spot y on the canvas.
    pub hotspot_y: f64,
    /// Canvas size (square).
    pub canvas: f64,
    /// Outline color, drawn under the fill.
    pub outline_color: String,
    /// Outline stroke width in canvas units.
    pub outline_width: f64,
}

#[derive(serde::Deserialize)]
struct Doc {
    canvas: f64,
    outline: Outline,
    shapes: std::collections::BTreeMap<String, Entry>,
}

#[derive(serde::Deserialize)]
struct Outline {
    color: String,
    width: f64,
}

#[derive(serde::Deserialize)]
struct Entry {
    hotspot: [f64; 2],
    d: String,
}

fn table() -> &'static Vec<CursorArt> {
    static TABLE: OnceLock<Vec<CursorArt>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let doc: Doc = serde_json::from_str(CURSOR_ART_JSON).expect("presence-cursors.json");
        CursorShape::ALL
            .iter()
            .map(|shape| {
                let e = doc
                    .shapes
                    .get(shape.as_str())
                    .unwrap_or_else(|| panic!("no art for {}", shape.as_str()));
                CursorArt {
                    shape: *shape,
                    path_d: e.d.clone(),
                    hotspot_x: e.hotspot[0],
                    hotspot_y: e.hotspot[1],
                    canvas: doc.canvas,
                    outline_color: doc.outline.color.clone(),
                    outline_width: doc.outline.width,
                }
            })
            .collect()
    })
}

/// The art for `shape`.
pub fn cursor_art(shape: CursorShape) -> &'static CursorArt {
    &table()[(shape.to_wire() - 1) as usize]
}

/// The art for every shape, in wire order.
pub fn all_cursor_art() -> &'static [CursorArt] {
    table()
}

/// A standalone SVG of `shape` filled with `color`, `size` pixels square.
pub fn cursor_art_svg(shape: CursorShape, color: &str, size: f64) -> String {
    let a = cursor_art(shape);
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{size}" height="{size}" viewBox="0 0 {c} {c}"><path d="{d}" fill="{color}" stroke="{oc}" stroke-width="{ow}" stroke-linejoin="round" paint-order="stroke"/></svg>"#,
        c = a.canvas,
        d = a.path_d,
        oc = a.outline_color,
        ow = a.outline_width,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shape_has_art_on_the_canvas() {
        for shape in CursorShape::ALL {
            let a = cursor_art(shape);
            assert_eq!(a.shape, shape);
            assert!(a.path_d.starts_with('M'), "{shape:?}");
            assert!(a.path_d.chars().all(|c| "MLCZ -.0123456789".contains(c)));
            assert!((0.0..=a.canvas).contains(&a.hotspot_x));
            assert!((0.0..=a.canvas).contains(&a.hotspot_y));
        }
        assert_eq!(all_cursor_art().len(), 14);
        assert!(cursor_art_svg(CursorShape::Text, "#e6194b", 32.0).contains("fill=\"#e6194b\""));
    }
}
