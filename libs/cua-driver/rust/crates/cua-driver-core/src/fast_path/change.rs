// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What changed after a sparse-target action: compare low-resolution
//! thumbnails taken before and after, and return the padded box of the
//! changed tiles (in full-resolution window pixels).

use super::pixel::GrayImage;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChangeRegion {
    /// Nothing changed: the click may have done nothing.
    Unchanged,
    /// `[x, y, w, h]` in full-resolution window pixels.
    Region {
        x: usize,
        y: usize,
        w: usize,
        h: usize,
    },
    /// More than `whole_fraction` of the window changed.
    Whole,
}

#[derive(Clone, Copy, Debug)]
pub struct ChangeConfig {
    /// Thumbnail scale divisor (8 = one-eighth).
    pub divisor: usize,
    /// Tile side in thumbnail pixels.
    pub tile: usize,
    /// Mean per-tile difference (0 to 255) above which a tile changed.
    pub threshold: f64,
    /// Padding around the box, in full-resolution pixels.
    pub pad: usize,
    pub whole_fraction: f64,
}

impl Default for ChangeConfig {
    fn default() -> Self {
        Self {
            divisor: 8,
            tile: 4,
            threshold: 6.0,
            pad: 16,
            whole_fraction: 0.4,
        }
    }
}

/// Make the one-eighth thumbnail of a full window image.
pub fn thumbnail(full: &GrayImage, cfg: &ChangeConfig) -> GrayImage {
    full.resize(
        (full.width / cfg.divisor).max(1),
        (full.height / cfg.divisor).max(1),
    )
}

pub fn changed_region(
    before: &GrayImage,
    after: &GrayImage,
    full_w: usize,
    full_h: usize,
    cfg: &ChangeConfig,
) -> ChangeRegion {
    if before.width != after.width || before.height != after.height {
        return ChangeRegion::Whole;
    }
    let (w, h, t) = (before.width, before.height, cfg.tile.max(1));
    let (tiles_x, tiles_y) = (w.div_ceil(t), h.div_ceil(t));
    let mut changed = 0usize;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (usize::MAX, usize::MAX, 0usize, 0usize);
    for ty in 0..tiles_y {
        for tx in 0..tiles_x {
            let (x0, y0) = (tx * t, ty * t);
            let (x1, y1) = ((x0 + t).min(w), (y0 + t).min(h));
            let mut sum = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += (before.get(x, y) as i32 - after.get(x, y) as i32).unsigned_abs();
                }
            }
            if sum as f64 / ((x1 - x0) * (y1 - y0)) as f64 > cfg.threshold {
                changed += 1;
                min_x = min_x.min(x0);
                min_y = min_y.min(y0);
                max_x = max_x.max(x1);
                max_y = max_y.max(y1);
            }
        }
    }
    if changed == 0 {
        return ChangeRegion::Unchanged;
    }
    let sx = full_w as f64 / w as f64;
    let sy = full_h as f64 / h as f64;
    let x = ((min_x as f64 * sx) as usize).saturating_sub(cfg.pad);
    let y = ((min_y as f64 * sy) as usize).saturating_sub(cfg.pad);
    let x1 = ((max_x as f64 * sx) as usize + cfg.pad).min(full_w);
    let y1 = ((max_y as f64 * sy) as usize + cfg.pad).min(full_h);
    if ((x1 - x) * (y1 - y)) as f64 > cfg.whole_fraction * (full_w * full_h) as f64 {
        return ChangeRegion::Whole;
    }
    ChangeRegion::Region {
        x,
        y,
        w: x1 - x,
        h: y1 - y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_changed() {
        let a = GrayImage::filled(100, 80, 120);
        assert_eq!(
            changed_region(&a, &a.clone(), 800, 640, &ChangeConfig::default()),
            ChangeRegion::Unchanged
        );
    }

    #[test]
    fn a_small_change_maps_back_to_full_resolution() {
        let a = GrayImage::filled(100, 80, 120);
        let mut b = a.clone();
        b.fill_rect(40, 20, 8, 8, 255);
        match changed_region(&a, &b, 800, 640, &ChangeConfig::default()) {
            ChangeRegion::Region { x, y, w, h } => {
                assert!(x <= 320 && x + w >= 384, "{x} {w}");
                assert!(y <= 160 && y + h >= 224, "{y} {h}");
                assert!(w < 200 && h < 200);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_page_change_is_whole() {
        let a = GrayImage::filled(100, 80, 120);
        let b = GrayImage::filled(100, 80, 10);
        assert_eq!(
            changed_region(&a, &b, 800, 640, &ChangeConfig::default()),
            ChangeRegion::Whole
        );
    }

    #[test]
    fn thumbnail_is_one_eighth() {
        let full = GrayImage::filled(800, 640, 1);
        let t = thumbnail(&full, &ChangeConfig::default());
        assert_eq!((t.width, t.height), (100, 80));
    }
}
