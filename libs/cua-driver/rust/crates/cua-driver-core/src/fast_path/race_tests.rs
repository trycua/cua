// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Race tests over a simulated window: the UI changes between the read and
//! the act, and the guards must refuse or relocate, never act on the wrong
//! target. Platform fixtures repeat these cases against real apps.

use super::fingerprint::{
    verify_element, Delivery, ElementFingerprint, ElementObservation, HitOutcome,
};
use super::pixel::{GrayImage, Tolerance};
use super::pixel_guard::{check, CropSource, PixelGuardConfig};
use super::reason::{Reason, Verdict};
use super::Rect;

/// xorshift, so the tests are deterministic.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[derive(Clone, Debug)]
struct Elem {
    /// Identity of the live reference (what AXPress acts on).
    id: u32,
    /// What it shows (what the agent meant to click).
    content: u32,
    frame: Rect,
    alive: bool,
}

#[derive(Clone, Debug)]
struct Ui {
    elems: Vec<Elem>,
}

impl Ui {
    fn new() -> Self {
        let elems = (0..6)
            .map(|i| Elem {
                id: i,
                content: 100 + i,
                frame: Rect::new(
                    20.0 + (i % 3) as f64 * 120.0,
                    40.0 + (i / 3) as f64 * 60.0,
                    80.0,
                    24.0,
                ),
                alive: true,
            })
            .collect();
        Self { elems }
    }
    fn label(content: u32) -> String {
        format!("Button {content}")
    }
    fn fingerprint(&self, i: usize) -> ElementFingerprint {
        let e = &self.elems[i];
        ElementFingerprint {
            role: "AXButton".into(),
            label: Some(Self::label(e.content)),
            frame: Some(e.frame),
            ..Default::default()
        }
    }
    fn observe(&self, i: usize) -> ElementObservation {
        let e = &self.elems[i];
        ElementObservation {
            valid: e.alive,
            role: e.alive.then(|| "AXButton".into()),
            label: e.alive.then(|| Self::label(e.content)),
            frame: e.alive.then_some(e.frame),
            ..Default::default()
        }
    }
    fn at(&self, x: f64, y: f64) -> Option<usize> {
        self.elems
            .iter()
            .rposition(|e| e.alive && e.frame.contains(x, y))
    }
    fn hit(&self, target: usize, x: f64, y: f64) -> HitOutcome {
        match self.at(x, y) {
            Some(i) if i == target => HitOutcome::Target,
            Some(_) => HitOutcome::Other,
            None => HitOutcome::Nothing,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Change {
    None,
    Move,
    SmallMove,
    Swap,
    Remove,
    RowReuse,
    Cover,
}

fn apply(ui: &mut Ui, target: usize, change: Change, rng: &mut Rng) {
    let other = (target + 1 + rng.below(5) as usize) % 6;
    match change {
        Change::None => {}
        Change::Move => {
            ui.elems[target].frame.x += 40.0 + rng.below(80) as f64;
            ui.elems[target].frame.y += rng.below(30) as f64;
        }
        Change::SmallMove => ui.elems[target].frame.x += 1.0,
        Change::Swap => {
            let a = ui.elems[target].frame;
            ui.elems[target].frame = ui.elems[other].frame;
            ui.elems[other].frame = a;
        }
        Change::Remove => ui.elems[target].alive = false,
        Change::RowReuse => ui.elems[target].content += 1000,
        Change::Cover => {
            // Another element is moved on top of the target.
            ui.elems[other].frame = ui.elems[target].frame;
            let moved = ui.elems.remove(other);
            ui.elems.push(moved);
        }
    }
}

const CHANGES: [Change; 7] = [
    Change::None,
    Change::Move,
    Change::SmallMove,
    Change::Swap,
    Change::Remove,
    Change::RowReuse,
    Change::Cover,
];

#[derive(Default, Debug)]
struct Tally {
    acted: u32,
    refused: u32,
    relocated: u32,
    wrong: u32,
    static_refusals: u32,
}

/// Act as the driver would: check, retry once on a pure move, then act.
fn act(
    ui: &Ui,
    target_id: u32,
    fp: &ElementFingerprint,
    delivery: Delivery,
    intended: u32,
    tally: &mut Tally,
) {
    let Some(target) = ui.elems.iter().position(|e| e.id == target_id) else {
        tally.refused += 1;
        return;
    };
    let (mut px, mut py) = fp.frame.unwrap().center();
    let hit = (delivery == Delivery::Pixel).then(|| ui.hit(target, px, py));
    let mut verdict = verify_element(fp, &ui.observe(target), delivery, hit);
    if let Verdict::Relocated { dx, dy } = verdict {
        tally.relocated += 1;
        px += dx;
        py += dy;
        // Re-check at the new point with a fresh hit-test; the moved frame
        // is the new reference frame.
        let mut moved = fp.clone();
        moved.frame = ui.observe(target).frame;
        verdict = verify_element(
            &moved,
            &ui.observe(target),
            delivery,
            Some(ui.hit(target, px, py)),
        );
        if !verdict.is_ok() {
            tally.refused += 1;
            return;
        }
    }
    if !verdict.is_ok() {
        tally.refused += 1;
        return;
    }
    tally.acted += 1;
    let pressed = match delivery {
        Delivery::Semantic => ui.elems[target].content,
        Delivery::Pixel => match ui.at(px, py) {
            Some(i) => ui.elems[i].content,
            None => 0,
        },
    };
    if pressed != intended {
        tally.wrong += 1;
    }
}

#[test]
fn randomized_race_never_acts_on_the_wrong_target() {
    let mut rng = Rng(0x5eed_cafe);
    for delivery in [Delivery::Semantic, Delivery::Pixel] {
        let mut tally = Tally::default();
        for _ in 0..1000 {
            let mut ui = Ui::new();
            let target = rng.below(6) as usize;
            let fp = ui.fingerprint(target);
            let intended = ui.elems[target].content;
            let id = ui.elems[target].id;
            let change = CHANGES[rng.below(CHANGES.len() as u64) as usize];
            apply(&mut ui, target, change, &mut rng);
            let before = tally.refused;
            act(&ui, id, &fp, delivery, intended, &mut tally);
            if matches!(change, Change::None | Change::SmallMove) && tally.refused > before {
                tally.static_refusals += 1;
            }
        }
        assert_eq!(tally.wrong, 0, "{delivery:?}: {tally:?}");
        assert_eq!(tally.static_refusals, 0, "{delivery:?}: {tally:?}");
        assert!(tally.refused > 0 && tally.acted > 0, "{tally:?}");
        if delivery == Delivery::Pixel {
            assert!(tally.relocated > 0, "{tally:?}");
        }
    }
}

#[test]
fn moved_button_semantic_presses_the_right_one_pixel_relocates() {
    let mut ui = Ui::new();
    let fp = ui.fingerprint(0);
    ui.elems[0].frame.y += 200.0; // into empty space below the grid
    let mut t = Tally::default();
    act(&ui, 0, &fp, Delivery::Semantic, 100, &mut t);
    act(&ui, 0, &fp, Delivery::Pixel, 100, &mut t);
    assert_eq!((t.acted, t.wrong, t.relocated), (2, 0, 1));
}

#[test]
fn swapped_buttons_pixel_refuses_semantic_presses_a() {
    let mut ui = Ui::new();
    let fp = ui.fingerprint(0);
    let a = ui.elems[0].frame;
    ui.elems[0].frame = ui.elems[3].frame;
    ui.elems[3].frame = a;
    // Swapped frames differ in y, so the guard sees a move of A and
    // relocates to A's new spot; it never clicks B at the old point.
    let mut t = Tally::default();
    act(&ui, 0, &fp, Delivery::Pixel, 100, &mut t);
    act(&ui, 0, &fp, Delivery::Semantic, 100, &mut t);
    assert_eq!(t.wrong, 0);
    // Same-frame swap (B now exactly where A was, A hidden under it).
    let mut ui = Ui::new();
    let fp = ui.fingerprint(0);
    apply(&mut ui, 0, Change::Cover, &mut Rng(1));
    let hit = ui.hit(
        0,
        fp.frame.unwrap().center().0,
        fp.frame.unwrap().center().1,
    );
    let v = verify_element(&fp, &ui.observe(0), Delivery::Pixel, Some(hit));
    assert_eq!(v.reason(), Some(Reason::HitMismatch));
}

#[test]
fn row_reuse_and_removal_refuse() {
    let mut ui = Ui::new();
    let fp = ui.fingerprint(2);
    ui.elems[2].content = 999;
    assert_eq!(
        verify_element(&fp, &ui.observe(2), Delivery::Semantic, None).reason(),
        Some(Reason::Relabelled)
    );
    ui.elems[2].alive = false;
    assert_eq!(
        verify_element(&fp, &ui.observe(2), Delivery::Semantic, None).reason(),
        Some(Reason::Gone)
    );
}

// ── Pixel-only (no tree) races ───────────────────────────────────────────

/// Renders tiles whose pattern encodes their content, on a textured canvas.
fn render(tiles: &[(i64, i64, u32)], seed: u32) -> GrayImage {
    let mut img = super::pixel::test_images::background(480, 320, seed);
    for &(x, y, content) in tiles {
        img.fill_rect(x, y, 48, 28, 30);
        for bit in 0..6 {
            if content >> bit & 1 == 1 {
                img.fill_rect(x + 4 + bit as i64 * 7, y + 4, 5, 20, 235);
            } else {
                img.fill_rect(x + 4 + bit as i64 * 7, y + 18, 5, 6, 140);
            }
        }
    }
    img
}

struct Static(GrayImage);
impl CropSource for Static {
    fn capture(&mut self, x: i64, y: i64, w: usize, h: usize) -> Option<GrayImage> {
        self.0.crop(x, y, w, h)
    }
    fn sleep_ms(&mut self, _: u64) {}
}

fn tile_at(tiles: &[(i64, i64, u32)], x: f64, y: f64) -> Option<u32> {
    tiles
        .iter()
        .rev()
        .find(|(tx, ty, _)| {
            x >= *tx as f64 && x < (*tx + 48) as f64 && y >= *ty as f64 && y < (*ty + 28) as f64
        })
        .map(|t| t.2)
}

#[test]
fn randomized_canvas_race_never_clicks_the_wrong_tile() {
    let mut rng = Rng(0xfeed_f00d);
    let cfg = PixelGuardConfig::for_scale(1.0, false);
    let (mut wrong, mut refused, mut relocated, mut static_refused) = (0, 0, 0, 0);
    for _ in 0..300 {
        let tiles: Vec<(i64, i64, u32)> =
            vec![(60, 60, 5), (220, 60, 9), (60, 200, 22), (300, 200, 41)];
        let target = rng.below(4) as usize;
        let read = render(&tiles, 3);
        let intended = tiles[target].2;
        let mut now = tiles.clone();
        let kind = rng.below(5);
        match kind {
            0 => {}
            1 => now[target].0 += 10 + rng.below(20) as i64, // shift within radius
            2 => now[target].2 ^= 0b1011,                    // redrawn as other content
            3 => now.swap(0, 1),                             // content order changes
            _ => {
                now[target].0 += 120; // moved far away
            }
        }
        // `swap` moves contents: positions stay, contents trade places.
        if kind == 3 {
            let (a, b) = (now[0].2, now[1].2);
            now[0] = (tiles[0].0, tiles[0].1, a);
            now[1] = (tiles[1].0, tiles[1].1, b);
        }
        let image = render(&now, 3);
        let point = (tiles[target].0 as f64 + 24.0, tiles[target].1 as f64 + 14.0);
        let r = check(&read, point, &cfg, &mut Static(image));
        let click = match r.verdict {
            Verdict::Ok => Some(point),
            Verdict::Relocated { dx, dy } => {
                relocated += 1;
                Some((point.0 + dx, point.1 + dy))
            }
            Verdict::Refuse { .. } => {
                refused += 1;
                if kind == 0 {
                    static_refused += 1;
                }
                None
            }
        };
        if let Some((x, y)) = click {
            if tile_at(&now, x, y) != Some(intended) {
                wrong += 1;
            }
        }
    }
    assert_eq!(wrong, 0);
    assert_eq!(static_refused, 0);
    assert!(refused > 0 && relocated > 0);
}

/// Alternates frames with and without a caret next to the target.
struct Blinking {
    base: GrayImage,
    rng: Rng,
}
impl CropSource for Blinking {
    fn capture(&mut self, x: i64, y: i64, w: usize, h: usize) -> Option<GrayImage> {
        let mut f = self.base.clone();
        if self.rng.below(2) == 0 {
            f.fill_rect(118, 62, 2, 18, 0);
        }
        f.crop(x, y, w, h)
    }
    fn sleep_ms(&mut self, _: u64) {}
}

#[test]
fn blinking_caret_does_not_cause_false_refusals() {
    let base = render(&[(60, 60, 5)], 9);
    let cfg = PixelGuardConfig::for_scale(1.0, false);
    let mut src = Blinking {
        base: base.clone(),
        rng: Rng(42),
    };
    let mut refusals = 0;
    for i in 0..1000 {
        // Half the reads were taken with the caret visible.
        let mut read = base.clone();
        if i % 2 == 0 {
            read.fill_rect(118, 62, 2, 18, 0);
        }
        if !check(&read, (84.0, 74.0), &cfg, &mut src).verdict.is_ok() {
            refusals += 1;
        }
    }
    assert!(refusals < 20, "{refusals} refusals in 1000 static clicks");
}

/// A remote-desktop stream: compression noise on every frame and a remote
/// cursor drawn in the content that wanders across the target.
struct RemoteStream {
    base: GrayImage,
    rng: Rng,
    cursor: (i64, i64),
}
impl CropSource for RemoteStream {
    fn capture(&mut self, x: i64, y: i64, w: usize, h: usize) -> Option<GrayImage> {
        let mut f = self.base.clone();
        for p in f.pixels.iter_mut() {
            let n = (self.rng.below(13) as i32) - 6;
            *p = (*p as i32 + n).clamp(0, 255) as u8;
        }
        // 8x8 block artefacts.
        for _ in 0..6 {
            let bx = self.rng.below(60) as i64 * 8;
            let by = self.rng.below(40) as i64 * 8;
            let shift = self.rng.below(20) as u8;
            for yy in by..by + 8 {
                for xx in bx..bx + 8 {
                    let i = yy as usize * f.width + xx as usize;
                    if i < f.pixels.len() {
                        f.pixels[i] = f.pixels[i].saturating_add(shift);
                    }
                }
            }
        }
        f.fill_rect(self.cursor.0, self.cursor.1, 12, 18, 0);
        f.crop(x, y, w, h)
    }
    fn sleep_ms(&mut self, _: u64) {}
}

#[test]
fn remote_desktop_noise_and_cursor_do_not_refuse_static_clicks() {
    let base = render(&[(200, 140, 13)], 21);
    let mut src = RemoteStream {
        base: base.clone(),
        rng: Rng(7),
        cursor: (210, 146),
    };
    let mut refusals = 0;
    for i in 0..200 {
        src.cursor = (190 + (i % 7) * 6, 136 + (i % 5) * 4);
        let mut cfg = PixelGuardConfig::for_scale(1.0, true);
        cfg.cursor_mask = Some((src.cursor.0 - 2, src.cursor.1 - 2, 16, 22));
        if !check(&base, (224.0, 154.0), &cfg, &mut src).verdict.is_ok() {
            refusals += 1;
        }
    }
    assert_eq!(refusals, 0);
}

#[test]
fn remote_desktop_real_change_still_refuses() {
    let read = render(&[(200, 140, 13)], 21);
    let changed = render(&[(200, 140, 50)], 21);
    let mut src = RemoteStream {
        base: changed,
        rng: Rng(8),
        cursor: (10, 10),
    };
    let cfg = PixelGuardConfig {
        tolerance: Tolerance::REMOTE,
        ..PixelGuardConfig::for_scale(1.0, true)
    };
    assert_eq!(
        check(&read, (224.0, 154.0), &cfg, &mut src)
            .verdict
            .reason(),
        Some(Reason::RegionChanged)
    );
}

#[test]
fn no_tree_window_gets_window_guard_and_pixel_check() {
    use super::classify::{classify, HitInfo, TargetClass, TreeStats};
    let win = Rect::new(0.0, 0.0, 480.0, 320.0);
    let hit = HitInfo {
        role: Some("AXWindow".into()),
        is_window: true,
        ..Default::default()
    };
    let class = classify(
        Some(&hit),
        Some(&TreeStats {
            elements: 1,
            thin_frames: 0,
        }),
        win,
    );
    assert_eq!(class, TargetClass::Sparse);
    assert!(class.needs_pixel_check());
}
