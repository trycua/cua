// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Pixel guard for sparse or no-tree targets (design note section 3).
//!
//! Compare a crop around the point from the read-time window image with
//! crops captured right before acting. Two act-time crops 50 ms apart tell
//! animation apart from a real change; a template match recovers a pure
//! shift.

use super::pixel::{diff, similar, template_match, CropDiff, GrayImage, Match, Tolerance};
use super::reason::{Reason, Verdict};

/// Captures pixels of the target window only (never the screen, so another
/// window on top cannot fool the check). Coordinates are window pixels.
pub trait CropSource {
    fn capture(&mut self, x: i64, y: i64, w: usize, h: usize) -> Option<GrayImage>;
    fn sleep_ms(&mut self, ms: u64);
}

#[derive(Clone, Debug)]
pub struct PixelGuardConfig {
    /// Crop side in window pixels (64 to 96 pt times the scale).
    pub crop: usize,
    /// Template-match search radius in window pixels (about 48 pt).
    pub search_radius: usize,
    pub stability_gap_ms: u64,
    pub settle_budget_ms: u64,
    pub tolerance: Tolerance,
    pub min_match: f64,
    /// A cursor drawn in the content, masked in every crop (window pixels).
    pub cursor_mask: Option<(i64, i64, usize, usize)>,
}

impl PixelGuardConfig {
    pub fn for_scale(scale: f64, remote: bool) -> Self {
        let s = scale.max(1.0);
        Self {
            crop: (80.0 * s).round() as usize,
            search_radius: (48.0 * s).round() as usize,
            stability_gap_ms: 50,
            settle_budget_ms: 300,
            tolerance: if remote {
                Tolerance::REMOTE
            } else {
                Tolerance::NATIVE
            },
            min_match: 0.9,
            cursor_mask: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PixelCheck {
    /// `Relocated` offsets are in window pixels.
    pub verdict: Verdict,
    pub captures: u32,
    pub waited_ms: u64,
    pub diff: Option<CropDiff>,
}

fn masked(
    mut img: GrayImage,
    origin: (i64, i64),
    mask: Option<(i64, i64, usize, usize)>,
) -> GrayImage {
    if let Some((mx, my, mw, mh)) = mask {
        img.fill_rect(mx - origin.0, my - origin.1, mw, mh, 128);
    }
    img
}

pub fn check(
    read_image: &GrayImage,
    point: (f64, f64),
    cfg: &PixelGuardConfig,
    src: &mut dyn CropSource,
) -> PixelCheck {
    let mut out = PixelCheck {
        verdict: Verdict::Ok,
        captures: 0,
        waited_ms: 0,
        diff: None,
    };
    let Some((read_crop, x0, y0)) = read_image.crop_centered(point.0, point.1, cfg.crop) else {
        out.verdict = Verdict::refuse(Reason::Offscreen, "the point is outside the window image");
        return out;
    };
    let size = read_crop.width;
    let read_crop = masked(read_crop, (x0, y0), cfg.cursor_mask);

    let grab = |src: &mut dyn CropSource, out: &mut PixelCheck| {
        out.captures += 1;
        src.capture(x0, y0, size, size)
            .map(|c| masked(c, (x0, y0), cfg.cursor_mask))
    };

    let Some(mut a) = grab(src, &mut out) else {
        out.verdict = Verdict::refuse(Reason::NotVisible, "the window could not be captured");
        return out;
    };
    loop {
        src.sleep_ms(cfg.stability_gap_ms);
        out.waited_ms += cfg.stability_gap_ms;
        let Some(b) = grab(src, &mut out) else {
            out.verdict = Verdict::refuse(Reason::NotVisible, "the window could not be captured");
            return out;
        };
        if similar(&a, &b, &cfg.tolerance) {
            a = b;
            break;
        }
        if out.waited_ms >= cfg.settle_budget_ms {
            out.verdict = Verdict::refuse(
                Reason::Animating,
                format!(
                    "the target area did not settle within {} ms",
                    cfg.settle_budget_ms
                ),
            );
            return out;
        }
        a = b;
    }

    let d = diff(&read_crop, &a, &cfg.tolerance);
    out.diff = Some(d);
    if d.same(&cfg.tolerance) {
        return out;
    }

    // Stable but different: was it only shifted?
    let quarter = (size / 4) as i64;
    let tpl = read_crop
        .crop(quarter, quarter, size / 2, size / 2)
        .expect("centre half is inside the crop");
    let r = cfg.search_radius as i64;
    let sx = (x0 + quarter - r).max(0);
    let sy = (y0 + quarter - r).max(0);
    let sx1 = (x0 + quarter + tpl.width as i64 + r).min(read_image.width as i64);
    let sy1 = (y0 + quarter + tpl.height as i64 + r).min(read_image.height as i64);
    let search = if sx1 > sx && sy1 > sy {
        out.captures += 1;
        src.capture(sx, sy, (sx1 - sx) as usize, (sy1 - sy) as usize)
            .map(|c| masked(c, (sx, sy), cfg.cursor_mask))
    } else {
        None
    };
    let detail = format!(
        "{:.0}% of the area changed (mean difference {:.2})",
        d.changed_fraction * 100.0,
        d.mean_diff
    );
    out.verdict = match search.map(|s| template_match(&tpl, &s, cfg.min_match)) {
        Some(Match::Unique { x, y, .. }) => {
            let dx = (sx + x - (x0 + quarter)) as f64;
            let dy = (sy + y - (y0 + quarter)) as f64;
            if dx == 0.0 && dy == 0.0 {
                Verdict::refuse(Reason::RegionChanged, detail)
            } else {
                Verdict::Relocated { dx, dy }
            }
        }
        Some(Match::Ambiguous) => Verdict::refuse(
            Reason::RegionChanged,
            format!("{detail}; the target matches more than one place nearby"),
        ),
        _ => Verdict::refuse(Reason::RegionChanged, detail),
    };
    out
}

/// A `CropSource` over a sequence of whole-window frames, for tests and the
/// shadow replay: each capture reads the current frame, then advances.
#[cfg(test)]
pub(crate) struct FrameSequence {
    pub frames: Vec<GrayImage>,
    pub index: usize,
    pub slept: u64,
    /// Advance one frame every N ms of sleep (0 = advance per capture).
    pub frame_ms: u64,
}

#[cfg(test)]
impl CropSource for FrameSequence {
    fn capture(&mut self, x: i64, y: i64, w: usize, h: usize) -> Option<GrayImage> {
        let i = match self.slept.checked_div(self.frame_ms) {
            Some(frame) => (frame as usize).min(self.frames.len() - 1),
            None => {
                let i = self.index.min(self.frames.len() - 1);
                self.index += 1;
                i
            }
        };
        self.frames[i].crop(x, y, w, h)
    }
    fn sleep_ms(&mut self, ms: u64) {
        self.slept += ms;
    }
}

#[cfg(test)]
mod tests {
    use super::super::pixel::test_images::*;
    use super::*;

    fn seq(frames: Vec<GrayImage>, frame_ms: u64) -> FrameSequence {
        FrameSequence {
            frames,
            index: 0,
            slept: 0,
            frame_ms,
        }
    }

    fn canvas_with_button(x: i64) -> GrayImage {
        let mut img = background(400, 300, 11);
        draw_button(&mut img, x, 140);
        img
    }

    #[test]
    fn unchanged_canvas_passes_with_two_captures() {
        let read = canvas_with_button(100);
        let mut src = seq(vec![read.clone()], 0);
        let r = check(
            &read,
            (120.0, 150.0),
            &PixelGuardConfig::for_scale(1.0, false),
            &mut src,
        );
        assert_eq!(r.verdict, Verdict::Ok);
        assert_eq!(r.captures, 2);
        assert_eq!(r.waited_ms, 50);
    }

    #[test]
    fn moving_canvas_target_is_relocated_not_clicked_at_old_spot() {
        let read = canvas_with_button(100);
        let now = canvas_with_button(140); // redrawn 40 px away
        let mut src = seq(vec![now], 0);
        let r = check(
            &read,
            (120.0, 150.0),
            &PixelGuardConfig::for_scale(1.0, false),
            &mut src,
        );
        assert_eq!(r.verdict, Verdict::Relocated { dx: 40.0, dy: 0.0 });
    }

    #[test]
    fn redrawn_as_another_shape_is_region_changed() {
        let read = canvas_with_button(100);
        let mut now = background(400, 300, 11);
        draw_circle_like(&mut now, 100, 140);
        let mut src = seq(vec![now], 0);
        let r = check(
            &read,
            (120.0, 150.0),
            &PixelGuardConfig::for_scale(1.0, false),
            &mut src,
        );
        assert_eq!(r.verdict.reason(), Some(Reason::RegionChanged));
    }

    #[test]
    fn a_spinner_that_never_settles_is_animating() {
        let read = canvas_with_button(100);
        let frames: Vec<GrayImage> = (0..20)
            .map(|i| {
                let mut f = canvas_with_button(100);
                f.fill_rect(
                    104 + (i % 4) * 8,
                    120,
                    8,
                    8,
                    if i % 2 == 0 { 0 } else { 255 },
                );
                f.fill_rect(80, 100 + (i % 3) * 20, 60, 20, (i * 50 % 255) as u8);
                f
            })
            .collect();
        let mut src = seq(frames, 0);
        let r = check(
            &read,
            (120.0, 150.0),
            &PixelGuardConfig::for_scale(1.0, false),
            &mut src,
        );
        assert_eq!(r.verdict.reason(), Some(Reason::Animating));
        assert!(r.waited_ms <= 300);
    }

    #[test]
    fn an_animation_that_settles_is_checked_after_it_settles() {
        let read = canvas_with_button(100);
        // Moving for 3 frames, then at rest 40 px right.
        let frames = vec![
            canvas_with_button(110),
            canvas_with_button(120),
            canvas_with_button(130),
            canvas_with_button(140),
        ];
        let mut src = seq(frames, 50);
        let r = check(
            &read,
            (120.0, 150.0),
            &PixelGuardConfig::for_scale(1.0, false),
            &mut src,
        );
        assert_eq!(r.verdict, Verdict::Relocated { dx: 40.0, dy: 0.0 });
    }

    #[test]
    fn uncapturable_window_is_not_visible() {
        struct Nothing;
        impl CropSource for Nothing {
            fn capture(&mut self, _: i64, _: i64, _: usize, _: usize) -> Option<GrayImage> {
                None
            }
            fn sleep_ms(&mut self, _: u64) {}
        }
        let read = canvas_with_button(100);
        let r = check(
            &read,
            (120.0, 150.0),
            &PixelGuardConfig::for_scale(1.0, false),
            &mut Nothing,
        );
        assert_eq!(r.verdict.reason(), Some(Reason::NotVisible));
    }

    #[test]
    fn hidpi_crop_scales_with_the_window() {
        let cfg = PixelGuardConfig::for_scale(2.0, false);
        assert_eq!(cfg.crop, 160);
        assert_eq!(cfg.search_radius, 96);
    }
}
