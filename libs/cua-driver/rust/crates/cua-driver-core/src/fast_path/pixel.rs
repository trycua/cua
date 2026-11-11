// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Greyscale crops, a perceptual hash, a normalised difference and a
//! template match. Small and allocation-light: crops are about 64 to 96 pt.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrayImage {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl GrayImage {
    pub fn new(width: usize, height: usize, pixels: Vec<u8>) -> Option<Self> {
        (width > 0 && height > 0 && pixels.len() == width * height).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn filled(width: usize, height: usize, value: u8) -> Self {
        Self {
            width,
            height,
            pixels: vec![value; width * height],
        }
    }

    /// From tightly packed RGBA or BGRA (luma ignores the R/B order to
    /// within a few levels, which the tolerances absorb).
    pub fn from_rgba(width: usize, height: usize, rgba: &[u8]) -> Option<Self> {
        Self::from_strided(width, height, rgba, width * 4)
    }

    /// From 4-byte pixels with a row stride (CGImage, BGRA DIBs, XImage).
    pub fn from_strided(width: usize, height: usize, data: &[u8], stride: usize) -> Option<Self> {
        if width == 0
            || height == 0
            || stride < width * 4
            || data.len() < stride * (height - 1) + width * 4
        {
            return None;
        }
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            let row = &data[y * stride..y * stride + width * 4];
            for px in row.chunks_exact(4) {
                let l = (px[0] as u32 * 77 + px[1] as u32 * 150 + px[2] as u32 * 29) >> 8;
                pixels.push(l as u8);
            }
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u8 {
        self.pixels[y * self.width + x]
    }

    /// Crop `w`x`h` at `(x, y)`; `None` unless the whole rect is inside.
    pub fn crop(&self, x: i64, y: i64, w: usize, h: usize) -> Option<GrayImage> {
        if x < 0 || y < 0 || w == 0 || h == 0 {
            return None;
        }
        let (x, y) = (x as usize, y as usize);
        if x + w > self.width || y + h > self.height {
            return None;
        }
        let mut pixels = Vec::with_capacity(w * h);
        for row in y..y + h {
            pixels.extend_from_slice(&self.pixels[row * self.width + x..row * self.width + x + w]);
        }
        Some(GrayImage {
            width: w,
            height: h,
            pixels,
        })
    }

    /// Crop a square of `size` centred on `(cx, cy)`, clamped into the image.
    pub fn crop_centered(&self, cx: f64, cy: f64, size: usize) -> Option<(GrayImage, i64, i64)> {
        let size = size.min(self.width).min(self.height);
        let x = ((cx - size as f64 / 2.0).round() as i64).clamp(0, (self.width - size) as i64);
        let y = ((cy - size as f64 / 2.0).round() as i64).clamp(0, (self.height - size) as i64);
        if !(0.0..self.width as f64).contains(&cx) || !(0.0..self.height as f64).contains(&cy) {
            return None;
        }
        self.crop(x, y, size, size).map(|c| (c, x, y))
    }

    /// Area-average resize.
    pub fn resize(&self, w: usize, h: usize) -> GrayImage {
        let mut out = Vec::with_capacity(w * h);
        for oy in 0..h {
            let y0 = oy * self.height / h;
            let y1 = ((oy + 1) * self.height / h).max(y0 + 1).min(self.height);
            for ox in 0..w {
                let x0 = ox * self.width / w;
                let x1 = ((ox + 1) * self.width / w).max(x0 + 1).min(self.width);
                let mut sum = 0u32;
                for y in y0..y1 {
                    for x in x0..x1 {
                        sum += self.get(x, y) as u32;
                    }
                }
                out.push((sum / ((y1 - y0) * (x1 - x0)) as u32) as u8);
            }
        }
        GrayImage {
            width: w,
            height: h,
            pixels: out,
        }
    }

    /// Paint a rect (used to mask an in-content cursor before comparing).
    pub fn fill_rect(&mut self, x: i64, y: i64, w: usize, h: usize, value: u8) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x + w as i64).max(0) as usize).min(self.width);
        let y1 = ((y + h as i64).max(0) as usize).min(self.height);
        for row in y0..y1 {
            for col in x0..x1 {
                self.pixels[row * self.width + col] = value;
            }
        }
    }
}

/// 64-bit difference hash (9x8 resize, compare horizontal neighbours).
pub fn dhash(img: &GrayImage) -> u64 {
    let small = img.resize(9, 8);
    let mut hash = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            hash <<= 1;
            if small.get(x, y) > small.get(x + 1, y) {
                hash |= 1;
            }
        }
    }
    hash
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropDiff {
    pub hash_distance: u32,
    /// Mean absolute difference at 32x32, 0 to 1.
    pub mean_diff: f64,
    /// Fraction of 32x32 cells whose difference is above the cell threshold.
    pub changed_fraction: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    pub cell_threshold: u8,
    pub max_changed_fraction: f64,
    pub max_mean_diff: f64,
    pub max_hash_distance: u32,
}

impl Tolerance {
    /// Native windows: a blinking caret or a focus ring is under 3% of the area.
    pub const NATIVE: Tolerance = Tolerance {
        cell_threshold: 40,
        max_changed_fraction: 0.03,
        max_mean_diff: 0.03,
        max_hash_distance: 14,
    };
    /// Lossy remote-desktop streams: wider, to absorb compression noise.
    pub const REMOTE: Tolerance = Tolerance {
        cell_threshold: 56,
        max_changed_fraction: 0.06,
        max_mean_diff: 0.06,
        max_hash_distance: 22,
    };
}

pub const COMPARE_SIZE: usize = 32;

pub fn diff(a: &GrayImage, b: &GrayImage, tol: &Tolerance) -> CropDiff {
    let sa = a.resize(COMPARE_SIZE, COMPARE_SIZE);
    let sb = b.resize(COMPARE_SIZE, COMPARE_SIZE);
    let mut total = 0u64;
    let mut changed = 0usize;
    for (pa, pb) in sa.pixels.iter().zip(&sb.pixels) {
        let d = (*pa as i32 - *pb as i32).unsigned_abs();
        total += d as u64;
        if d > tol.cell_threshold as u32 {
            changed += 1;
        }
    }
    let n = (COMPARE_SIZE * COMPARE_SIZE) as f64;
    CropDiff {
        hash_distance: (dhash(a) ^ dhash(b)).count_ones(),
        mean_diff: total as f64 / n / 255.0,
        changed_fraction: changed as f64 / n,
    }
}

impl CropDiff {
    /// Same within tolerance. The hash only counts when cells also changed,
    /// so flat areas whose hash bits flip on noise do not trip it.
    pub fn same(&self, tol: &Tolerance) -> bool {
        self.changed_fraction <= tol.max_changed_fraction
            && self.mean_diff <= tol.max_mean_diff
            && !(self.hash_distance > tol.max_hash_distance
                && self.changed_fraction > tol.max_changed_fraction / 3.0)
    }
}

pub fn similar(a: &GrayImage, b: &GrayImage, tol: &Tolerance) -> bool {
    diff(a, b, tol).same(tol)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Match {
    /// One clear match: top-left offset inside the search image and score.
    Unique { x: i64, y: i64, score: f64 },
    /// Two near-equal matches (repeated tiles, grid cells).
    Ambiguous,
    /// Nothing above the threshold, or a flat template.
    None,
}

/// NCC scores of `template` at every top-left offset in `[x0, x1] x [y0, y1]`.
fn ncc_scores(
    template: &GrayImage,
    search: &GrayImage,
    range: (usize, usize, usize, usize),
) -> Vec<(f64, i64, i64)> {
    let (tw, th) = (template.width, template.height);
    let n = (tw * th) as f64;
    let t_mean = template.pixels.iter().map(|p| *p as f64).sum::<f64>() / n;
    let t_dev: Vec<f64> = template.pixels.iter().map(|p| *p as f64 - t_mean).collect();
    let t_norm = t_dev.iter().map(|d| d * d).sum::<f64>().sqrt();
    let (x0, y0, x1, y1) = range;
    let mut scores = Vec::new();
    for oy in y0..=y1 {
        for ox in x0..=x1 {
            let (mut s_sum, mut s_sq, mut cross) = (0.0, 0.0, 0.0);
            for y in 0..th {
                let row =
                    &search.pixels[(oy + y) * search.width + ox..(oy + y) * search.width + ox + tw];
                let trow = &t_dev[y * tw..(y + 1) * tw];
                for (p, t) in row.iter().zip(trow) {
                    let v = *p as f64;
                    s_sum += v;
                    s_sq += v * v;
                    cross += v * t;
                }
            }
            // sum((s - mean_s) * t_dev) == sum(s * t_dev) because sum(t_dev) == 0.
            let var = (s_sq - s_sum * s_sum / n).max(0.0);
            let denom = t_norm * var.sqrt();
            scores.push((
                if denom > 0.0 { cross / denom } else { 0.0 },
                ox as i64,
                oy as i64,
            ));
        }
    }
    scores
}

fn flat(template: &GrayImage) -> bool {
    let n = template.pixels.len() as f64;
    let mean = template.pixels.iter().map(|p| *p as f64).sum::<f64>() / n;
    let var = template
        .pixels
        .iter()
        .map(|p| (*p as f64 - mean).powi(2))
        .sum::<f64>()
        / n;
    var.sqrt() < 4.0
}

/// Normalised cross-correlation of `template` over `search`. Coarse at half
/// resolution (and the uniqueness test there), then refined at full
/// resolution around the best coarse hit, so a 96 pt search stays a few ms.
pub fn template_match(template: &GrayImage, search: &GrayImage, min_score: f64) -> Match {
    let (tw, th) = (template.width, template.height);
    if tw > search.width || th > search.height || flat(template) {
        return Match::None;
    }
    let coarse = tw >= 24 && th >= 24;
    let (t, s, f) = if coarse {
        (
            template.resize(tw / 2, th / 2),
            search.resize(search.width / 2, search.height / 2),
            2i64,
        )
    } else {
        (template.clone(), search.clone(), 1i64)
    };
    if t.width > s.width || t.height > s.height {
        return Match::None;
    }
    let mut scores = ncc_scores(&t, &s, (0, 0, s.width - t.width, s.height - t.height));
    scores.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let Some(&(best, bx, by)) = scores.first() else {
        return Match::None;
    };
    if best < min_score - 0.1 {
        return Match::None;
    }
    let min_sep = (t.width.min(t.height) as i64 / 4).max(3);
    let rival = scores
        .iter()
        .skip(1)
        .find(|(_, x, y)| (x - bx).abs() > min_sep || (y - by).abs() > min_sep);
    if let Some(&(second, _, _)) = rival {
        if second >= best - 0.05 && second >= min_score - 0.1 {
            return Match::Ambiguous;
        }
    }
    let (cx, cy) = (bx * f, by * f);
    let r = f + 1;
    let x0 = (cx - r).max(0) as usize;
    let y0 = (cy - r).max(0) as usize;
    let x1 = ((cx + r) as usize).min(search.width - tw);
    let y1 = ((cy + r) as usize).min(search.height - th);
    let mut fine = ncc_scores(template, search, (x0, y0, x1.max(x0), y1.max(y0)));
    fine.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    match fine.first() {
        Some(&(score, x, y)) if score >= min_score => Match::Unique { x, y, score },
        _ => Match::None,
    }
}

#[cfg(test)]
pub(crate) mod test_images {
    use super::GrayImage;

    /// A textured background (so nothing is flat) with deterministic noise.
    pub fn background(w: usize, h: usize, seed: u32) -> GrayImage {
        let mut img = GrayImage::filled(w, h, 200);
        let mut s = seed.wrapping_mul(2654435761).max(1);
        for y in 0..h {
            for x in 0..w {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                let grain = (s % 9) as i32 - 4;
                let stripe = if (x / 16 + y / 16) % 2 == 0 { 0 } else { 12 };
                img.pixels[y * w + x] = (200 + grain - stripe) as u8;
            }
        }
        img
    }

    /// A "button": dark rounded-ish box with a light glyph bar, at (x, y).
    pub fn draw_button(img: &mut GrayImage, x: i64, y: i64) {
        img.fill_rect(x, y, 40, 20, 40);
        img.fill_rect(x + 6, y + 8, 18, 4, 230);
        img.fill_rect(x + 28, y + 4, 6, 12, 150);
    }

    /// A different shape at the same spot.
    pub fn draw_circle_like(img: &mut GrayImage, x: i64, y: i64) {
        img.fill_rect(x + 10, y, 20, 20, 90);
        img.fill_rect(x + 4, y + 6, 32, 8, 90);
    }
}

#[cfg(test)]
mod tests {
    use super::test_images::*;
    use super::*;

    #[test]
    fn identical_crops_are_same_and_hash_is_stable() {
        let a = background(80, 80, 1);
        let d = diff(&a, &a.clone(), &Tolerance::NATIVE);
        assert_eq!(d.hash_distance, 0);
        assert!(d.same(&Tolerance::NATIVE));
    }

    #[test]
    fn a_new_shape_is_different() {
        let mut a = background(80, 80, 1);
        draw_button(&mut a, 20, 30);
        let mut b = background(80, 80, 1);
        draw_circle_like(&mut b, 20, 30);
        assert!(!similar(&a, &b, &Tolerance::NATIVE));
    }

    #[test]
    fn a_blinking_caret_is_absorbed() {
        let a = background(80, 80, 3);
        let mut b = a.clone();
        b.fill_rect(50, 30, 2, 16, 0);
        assert!(
            similar(&a, &b, &Tolerance::NATIVE),
            "{:?}",
            diff(&a, &b, &Tolerance::NATIVE)
        );
    }

    #[test]
    fn a_thin_focus_ring_is_absorbed() {
        let a = background(96, 96, 4);
        let mut b = a.clone();
        b.fill_rect(20, 20, 56, 1, 90);
        b.fill_rect(20, 75, 56, 1, 90);
        assert!(similar(&a, &b, &Tolerance::NATIVE));
    }

    #[test]
    fn template_match_finds_a_shift() {
        let mut search = background(144, 144, 5);
        draw_button(&mut search, 70, 50);
        let mut read = background(144, 144, 5);
        draw_button(&mut read, 30, 50);
        let tpl = read.crop(24, 44, 52, 32).unwrap();
        match template_match(&tpl, &search, 0.9) {
            Match::Unique { x, y, .. } => assert_eq!((x, y), (64, 44)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn repeated_tiles_are_ambiguous() {
        let mut search = background(160, 60, 6);
        search.pixels.iter_mut().for_each(|p| *p = 200);
        draw_button(&mut search, 10, 20);
        draw_button(&mut search, 90, 20);
        let tpl = search.crop(6, 16, 48, 28).unwrap();
        assert_eq!(template_match(&tpl, &search, 0.9), Match::Ambiguous);
    }

    #[test]
    fn flat_template_matches_nothing() {
        let flat = GrayImage::filled(20, 20, 128);
        let search = background(60, 60, 7);
        assert_eq!(template_match(&flat, &search, 0.9), Match::None);
    }

    #[test]
    fn rgba_and_resize() {
        let rgba = vec![255u8; 4 * 4 * 4];
        let img = GrayImage::from_rgba(4, 4, &rgba).unwrap();
        assert!(img.pixels.iter().all(|p| *p >= 254));
        assert_eq!(img.resize(2, 2).pixels.len(), 4);
        assert!(GrayImage::from_rgba(4, 4, &rgba[..10]).is_none());
    }
}
