//! Canonical cursor-theme semantics and the embedded `cua.default` renderer.
//!
//! The default artwork was authored as Lottie on a 128×128 canvas. Production
//! rendering uses this bounded vector representation so the privileged overlay
//! never parses Lottie, ZIP, fonts, scripts, images, or network references.

use serde::{Deserialize, Serialize};
use tiny_skia::{Color, FillRule, Paint, Path, PathBuilder, Stroke, Transform};

pub use cua_driver_contract::{
    CursorAction, CursorDelivery as DeliveryModifier, CursorPlayback as PlaybackKind,
    CursorReducedMotion as ReducedMotion, CursorTarget as TargetModifier,
};

pub const DEFAULT_THEME_ID: &str = "cua.default";
pub const DEFAULT_THEME_VERSION: &str = "1.0.0";
pub const THEME_PROFILE: &str = "cua-driver-full-v1";
pub const CANVAS_SIZE: f32 = 128.0;
pub const DISPLAY_SIZE: f32 = 48.0;

fn ink() -> Color {
    Color::from_rgba8(255, 255, 255, 255)
}

fn outline() -> Color {
    Color::from_rgba8(255, 255, 255, 255)
}

pub const DEFAULT_CURSOR_FILL: [u8; 4] = [94, 192, 232, 255];

const SESSION_CURSOR_FILLS: &[[u8; 4]] = &[
    [178, 132, 255, 255],
    [247, 132, 170, 255],
    [96, 218, 174, 255],
    [244, 178, 66, 255],
    [76, 204, 224, 255],
    [221, 113, 236, 255],
    [232, 82, 98, 255],
    [184, 220, 54, 255],
    [80, 126, 236, 255],
];

/// Return the stable fill color for one session-owned cursor.
///
/// The anonymous/default cursor keeps the original Cua blue. Named sessions
/// hash into the former multi-cursor palette so concurrent runs are visually
/// distinct without accepting an agent-controlled styling argument.
pub fn session_fill_rgba(session_id: &str) -> [u8; 4] {
    if session_id.is_empty() || session_id == "default" {
        return DEFAULT_CURSOR_FILL;
    }

    SESSION_CURSOR_FILLS[stable_session_index(session_id, SESSION_CURSOR_FILLS.len())]
}

pub fn session_fill_hex(session_id: &str) -> String {
    let [r, g, b, _] = session_fill_rgba(session_id);
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn stable_session_index(id: &str, count: usize) -> usize {
    let suffix = id
        .rfind(['-', '_', '.'])
        .map(|index| &id[index + 1..])
        .unwrap_or(id);
    if let Ok(number) = suffix.parse::<usize>() {
        if number > 0 {
            return (number - 1) % count;
        }
    }
    if suffix.len() == 1 {
        if let Some(character) = suffix.chars().next() {
            if character.is_ascii_alphabetic() {
                return (character.to_ascii_lowercase() as usize - b'a' as usize) % count;
            }
        }
    }

    let mut hash: u32 = 2_166_136_261;
    for character in id.chars() {
        hash ^= character as u32;
        hash = hash.wrapping_mul(16_777_619);
    }
    hash as usize % count
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CursorVisualState {
    pub requested_action: CursorAction,
    pub resolved_action: CursorAction,
    pub delivery: Option<DeliveryModifier>,
    pub target: Option<TargetModifier>,
    pub elapsed_secs: f64,
    /// Grace period remaining after a short tool call ends, so observe/key
    /// cues remain visible for at least one rendered frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ending_secs: Option<f64>,
    pub reduced_motion: ReducedMotion,
    pub preempted_count: u64,
}

impl Default for CursorVisualState {
    fn default() -> Self {
        Self {
            requested_action: CursorAction::Idle,
            resolved_action: CursorAction::Idle,
            delivery: None,
            target: None,
            elapsed_secs: 0.0,
            ending_secs: None,
            reduced_motion: ReducedMotion::Auto,
            preempted_count: 0,
        }
    }
}

impl CursorVisualState {
    pub fn begin(
        &mut self,
        action: CursorAction,
        delivery: Option<DeliveryModifier>,
        target: Option<TargetModifier>,
    ) {
        if self.resolved_action != CursorAction::Idle && self.resolved_action != action {
            self.preempted_count = self.preempted_count.saturating_add(1);
        }
        self.requested_action = action;
        self.resolved_action = action;
        self.delivery = delivery;
        self.target = target;
        self.elapsed_secs = 0.0;
        self.ending_secs = None;
    }

    pub fn end(&mut self, action: CursorAction) {
        if self.resolved_action == action
            && matches!(action.playback(), PlaybackKind::Held | PlaybackKind::Loop)
        {
            self.ending_secs = Some(0.4);
        }
    }

    pub fn to_idle(&mut self) {
        self.requested_action = CursorAction::Idle;
        self.resolved_action = CursorAction::Idle;
        self.delivery = None;
        self.target = None;
        self.elapsed_secs = 0.0;
        self.ending_secs = None;
    }

    pub fn tick(&mut self, dt: f64) {
        let dt = dt.max(0.0);
        self.elapsed_secs = (self.elapsed_secs + dt).min(86_400.0);
        if let Some(remaining) = self.ending_secs {
            if remaining <= dt {
                self.to_idle();
                return;
            }
            self.ending_secs = Some(remaining - dt);
        }
        let action = self.resolved_action;
        if action.playback() == PlaybackKind::OneShot && self.elapsed_secs >= action.duration_secs()
        {
            self.to_idle();
        }
    }

    pub fn phase(&self) -> &'static str {
        match self.resolved_action.playback() {
            PlaybackKind::Resting | PlaybackKind::Loop => "loop",
            PlaybackKind::Held => "sustain",
            PlaybackKind::OneShot => "one_shot",
        }
    }

    pub fn frame(&self) -> u32 {
        let duration = self.resolved_action.duration_secs().max(1.0 / 30.0);
        let elapsed = match self.resolved_action.playback() {
            PlaybackKind::Resting | PlaybackKind::Loop | PlaybackKind::Held => {
                self.elapsed_secs.rem_euclid(duration)
            }
            PlaybackKind::OneShot => self.elapsed_secs.min(duration),
        };
        (elapsed * 30.0).floor() as u32
    }
}

fn solid_paint(color: Color, alpha: f32) -> Paint<'static> {
    Paint {
        shader: tiny_skia::Shader::SolidColor(
            Color::from_rgba(
                color.red(),
                color.green(),
                color.blue(),
                (color.alpha() * alpha).clamp(0.0, 1.0),
            )
            .unwrap_or(Color::TRANSPARENT),
        ),
        anti_alias: true,
        ..Default::default()
    }
}

fn cue_stroke(alpha: f32, width: f32) -> (Paint<'static>, Stroke) {
    (
        solid_paint(ink(), alpha),
        Stroke {
            width,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Default::default()
        },
    )
}

fn draw_path(
    pm: &mut tiny_skia::Pixmap,
    path: &Path,
    transform: Transform,
    alpha: f32,
    width: f32,
) {
    let (paint, stroke) = cue_stroke(alpha, width);
    pm.stroke_path(path, &paint, &stroke, transform, None);
}

fn draw_colored_path(
    pm: &mut tiny_skia::Pixmap,
    path: &Path,
    transform: Transform,
    alpha: f32,
    width: f32,
    fill_rgba: [u8; 4],
) {
    let stroke = Stroke {
        width,
        line_cap: tiny_skia::LineCap::Round,
        line_join: tiny_skia::LineJoin::Round,
        ..Default::default()
    };
    let color = Color::from_rgba8(fill_rgba[0], fill_rgba[1], fill_rgba[2], fill_rgba[3]);
    pm.stroke_path(path, &solid_paint(color, alpha), &stroke, transform, None);
}

fn draw_cue_glow(
    pm: &mut tiny_skia::Pixmap,
    path: &Path,
    transform: Transform,
    alpha: f32,
    width: f32,
    fill_rgba: [u8; 4],
) {
    const LAYERS: usize = 12;
    const OUTER_EXPANSION: f32 = 13.0;
    const INNER_EXPANSION: f32 = 2.5;
    const MAX_OPACITY: f32 = 0.17;

    let color = Color::from_rgba8(fill_rgba[0], fill_rgba[1], fill_rgba[2], fill_rgba[3]);
    let mut accumulated_opacity = 0.0;
    for layer in 0..LAYERS {
        let progress = (layer + 1) as f32 / LAYERS as f32;
        let expansion = OUTER_EXPANSION + (INNER_EXPANSION - OUTER_EXPANSION) * progress;
        let target_opacity = MAX_OPACITY * alpha.clamp(0.0, 1.0) * progress.powf(1.6);
        let layer_opacity =
            (target_opacity - accumulated_opacity) / (1.0 - accumulated_opacity).max(0.001);
        accumulated_opacity = target_opacity;
        if layer_opacity <= 0.0 {
            continue;
        }
        let stroke = Stroke {
            width: width + expansion,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Default::default()
        };
        pm.stroke_path(
            path,
            &solid_paint(color, layer_opacity),
            &stroke,
            transform,
            None,
        );
    }
}

fn draw_glowing_path(
    pm: &mut tiny_skia::Pixmap,
    path: &Path,
    transform: Transform,
    alpha: f32,
    width: f32,
    fill_rgba: [u8; 4],
) {
    draw_cue_glow(pm, path, transform, alpha, width, fill_rgba);
    draw_path(pm, path, transform, alpha, width + 1.5);
    draw_colored_path(
        pm,
        path,
        transform,
        alpha,
        (width - 1.0).max(1.5),
        fill_rgba,
    );
}

fn line_path(points: &[(f32, f32)]) -> Option<Path> {
    let (&(x, y), rest) = points.split_first()?;
    let mut builder = PathBuilder::new();
    builder.move_to(x, y);
    for &(x, y) in rest {
        builder.line_to(x, y);
    }
    builder.finish()
}

fn rounded_rect_path(x: f32, y: f32, width: f32, height: f32, radius: f32) -> Option<Path> {
    let r = radius.min(width * 0.5).min(height * 0.5).max(0.0);
    let k = 0.552_284_8;
    let mut builder = PathBuilder::new();
    builder.move_to(x + r, y);
    builder.line_to(x + width - r, y);
    builder.cubic_to(
        x + width - r + r * k,
        y,
        x + width,
        y + r - r * k,
        x + width,
        y + r,
    );
    builder.line_to(x + width, y + height - r);
    builder.cubic_to(
        x + width,
        y + height - r + r * k,
        x + width - r + r * k,
        y + height,
        x + width - r,
        y + height,
    );
    builder.line_to(x + r, y + height);
    builder.cubic_to(
        x + r - r * k,
        y + height,
        x,
        y + height - r + r * k,
        x,
        y + height - r,
    );
    builder.line_to(x, y + r);
    builder.cubic_to(x, y + r - r * k, x + r - r * k, y, x + r, y);
    builder.close();
    builder.finish()
}

fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn triangle_wave(t: f32) -> f32 {
    let t = t.rem_euclid(1.0);
    if t < 0.5 {
        t * 2.0
    } else {
        (1.0 - t) * 2.0
    }
}

/// Paint one frame of the embedded Full-v1 theme.
///
/// `anchor_x/y` is the existing overlay's cursor centre. The Lottie canvas is
/// centred there so the established click offset and path physics remain
/// unchanged. `heading` rotates the artwork around the canvas centre.
pub fn paint_default_theme(
    pm: &mut tiny_skia::Pixmap,
    visual: &CursorVisualState,
    anchor_x: f32,
    anchor_y: f32,
    heading: f32,
    backing_scale: f32,
    alpha: f32,
) {
    paint_default_theme_with_fill(
        pm,
        visual,
        anchor_x,
        anchor_y,
        heading,
        backing_scale,
        alpha,
        DEFAULT_CURSOR_FILL,
    );
}

pub fn paint_default_theme_with_fill(
    pm: &mut tiny_skia::Pixmap,
    visual: &CursorVisualState,
    anchor_x: f32,
    anchor_y: f32,
    heading: f32,
    backing_scale: f32,
    alpha: f32,
    fill_rgba: [u8; 4],
) {
    let scale = DISPLAY_SIZE * backing_scale / CANVAS_SIZE;
    let base_rotation = heading - std::f32::consts::FRAC_PI_4;
    let mut body_dx = 0.0;
    let mut body_dy = 0.0;
    let mut body_rotation = 0.0;
    let mut body_scale = 1.0;

    let duration = visual.resolved_action.duration_secs() as f32;
    let local = (visual.elapsed_secs as f32).rem_euclid(duration.max(1.0 / 30.0));
    let progress = (local / duration.max(1.0 / 30.0)).clamp(0.0, 1.0);
    let reduced = visual.reduced_motion == ReducedMotion::On;

    if !reduced {
        match visual.resolved_action {
            CursorAction::Idle => {
                let angle = progress * std::f32::consts::TAU;
                body_dx = angle.sin() * 5.0;
                body_dy = 6.0 * angle.cos() - 5.0;
                body_rotation = 2.5_f32.to_radians() * angle.cos();
            }
            CursorAction::Click => {
                body_scale = if progress < 0.35 {
                    1.0 - ease_in_out(progress / 0.35) * 0.07
                } else if progress < 0.6 {
                    0.93 + ease_in_out((progress - 0.35) / 0.25) * 0.10
                } else {
                    1.03 - ease_in_out((progress - 0.6) / 0.4) * 0.03
                };
            }
            CursorAction::Drag => {
                let held = ease_in_out(triangle_wave(progress));
                body_dx = held * 7.0;
                body_dy = held * 3.0;
            }
            _ => {}
        }
    }

    let canvas_transform = Transform::from_translate(-64.0, -64.0)
        .post_scale(scale, scale)
        .post_rotate(base_rotation.to_degrees())
        .post_translate(anchor_x, anchor_y);
    let body_transform = Transform::from_translate(-64.0, -64.0)
        .post_scale(scale * body_scale, scale * body_scale)
        .post_rotate((base_rotation + body_rotation).to_degrees())
        .post_translate(anchor_x + body_dx * scale, anchor_y + body_dy * scale);

    draw_cursor_glow(pm, body_transform, alpha, fill_rgba);
    draw_action_cue(
        pm,
        visual.resolved_action,
        progress,
        canvas_transform,
        alpha,
        reduced,
        fill_rgba,
    );
    draw_cursor_body(pm, body_transform, alpha, fill_rgba);
    draw_modifiers(pm, visual, canvas_transform, alpha, fill_rgba);
}

fn cursor_body_path() -> Option<Path> {
    let mut builder = PathBuilder::new();
    builder.move_to(55.0, 30.0);
    builder.cubic_to(48.0, 28.0, 42.0, 33.0, 43.0, 41.0);
    builder.line_to(64.0, 98.0);
    builder.cubic_to(67.0, 106.0, 73.0, 106.0, 77.0, 99.0);
    builder.line_to(86.0, 79.0);
    builder.cubic_to(88.0, 75.0, 91.0, 72.0, 95.0, 70.0);
    builder.line_to(108.0, 63.0);
    builder.cubic_to(115.0, 59.0, 114.0, 53.0, 107.0, 50.0);
    builder.close();
    builder.finish()
}

fn draw_cursor_glow(
    pm: &mut tiny_skia::Pixmap,
    transform: Transform,
    alpha: f32,
    fill_rgba: [u8; 4],
) {
    const LAYERS: usize = 36;
    const OUTER_WIDTH: f32 = 44.0;
    const INNER_WIDTH: f32 = 7.0;
    const MAX_OPACITY: f32 = 0.34;

    let Some(path) = cursor_body_path() else {
        return;
    };
    let color = Color::from_rgba8(fill_rgba[0], fill_rgba[1], fill_rgba[2], fill_rgba[3]);

    let mut accumulated_opacity = 0.0;
    for layer in 0..LAYERS {
        let progress = (layer + 1) as f32 / LAYERS as f32;
        let width = OUTER_WIDTH + (INNER_WIDTH - OUTER_WIDTH) * progress;
        let target_opacity = MAX_OPACITY * alpha.clamp(0.0, 1.0) * progress.powf(1.65);
        let layer_opacity =
            (target_opacity - accumulated_opacity) / (1.0 - accumulated_opacity).max(0.001);
        accumulated_opacity = target_opacity;
        if layer_opacity <= 0.0 {
            continue;
        }

        let stroke = Stroke {
            width,
            line_join: tiny_skia::LineJoin::Round,
            line_cap: tiny_skia::LineCap::Round,
            ..Default::default()
        };
        pm.stroke_path(
            &path,
            &solid_paint(color, layer_opacity),
            &stroke,
            transform,
            None,
        );
    }
}

fn draw_cursor_body(
    pm: &mut tiny_skia::Pixmap,
    transform: Transform,
    alpha: f32,
    fill_rgba: [u8; 4],
) {
    let Some(path) = cursor_body_path() else {
        return;
    };
    pm.fill_path(
        &path,
        &solid_paint(
            Color::from_rgba8(fill_rgba[0], fill_rgba[1], fill_rgba[2], fill_rgba[3]),
            alpha,
        ),
        FillRule::Winding,
        transform,
        None,
    );
    let paint = solid_paint(outline(), alpha);
    let mut stroke = Stroke {
        width: 5.0,
        ..Default::default()
    };
    stroke.line_join = tiny_skia::LineJoin::Round;
    pm.stroke_path(&path, &paint, &stroke, transform, None);
}

fn draw_action_cue(
    pm: &mut tiny_skia::Pixmap,
    action: CursorAction,
    progress: f32,
    transform: Transform,
    alpha: f32,
    reduced: bool,
    fill_rgba: [u8; 4],
) {
    let wave = if reduced {
        0.5
    } else {
        triangle_wave(progress)
    };
    match action {
        CursorAction::Idle => {}
        CursorAction::Observe => {
            let opacity = alpha
                * if reduced {
                    1.0
                } else {
                    (progress * 7.0).min(1.0)
                };
            let scale = if reduced { 1.0 } else { 0.88 + progress * 0.20 };
            let t = Transform::from_translate(-64.0, -64.0)
                .post_scale(scale, scale)
                .post_translate(64.0 + 8.0, 64.0 - 10.0)
                .post_concat(transform);
            let mut a = PathBuilder::new();
            a.move_to(38.0, 28.0);
            a.cubic_to(27.0, 29.0, 20.0, 38.0, 20.0, 49.0);
            if let Some(path) = a.finish() {
                draw_glowing_path(pm, &path, t, opacity, 4.0, fill_rgba);
            }
            let mut b = PathBuilder::new();
            b.move_to(42.0, 19.0);
            b.cubic_to(23.0, 19.0, 11.0, 33.0, 11.0, 51.0);
            if let Some(path) = b.finish() {
                draw_glowing_path(pm, &path, t, opacity, 4.0, fill_rgba);
            }
        }
        CursorAction::Click => {
            let cue_progress = (progress / 0.65).clamp(0.0, 1.0);
            let opacity = alpha * (1.0 - cue_progress).min(cue_progress * 4.0).clamp(0.0, 1.0);
            let cue_scale = 0.25 + ease_in_out(cue_progress) * 1.0;
            let t = Transform::from_translate(-25.0, -25.0)
                .post_scale(cue_scale, cue_scale)
                .post_translate(25.0 + 10.0, 25.0 + 3.0)
                .post_concat(transform);
            for points in [
                &[(35.0, 20.0), (34.0, 11.0)][..],
                &[(27.0, 25.0), (19.0, 19.0)][..],
                &[(25.0, 34.0), (15.0, 34.0)][..],
            ] {
                if let Some(path) = line_path(points) {
                    draw_glowing_path(pm, &path, t, opacity, 4.0, fill_rgba);
                }
            }
        }
        CursorAction::Drag => {
            let offset = if reduced {
                0.0
            } else {
                ease_in_out(wave) * 7.0
            };
            let t = Transform::from_translate(offset, offset * 0.43).post_concat(transform);
            for points in [
                &[(28.0, 38.0), (16.0, 35.0)][..],
                &[(26.0, 48.0), (12.0, 45.0)][..],
            ] {
                if let Some(path) = line_path(points) {
                    draw_glowing_path(pm, &path, t, alpha * (0.2 + wave * 0.8), 4.0, fill_rgba);
                }
            }
        }
        CursorAction::Scroll => {
            let y = if reduced { 0.0 } else { 4.0 - wave * 8.0 };
            let t = Transform::from_translate(-5.0, y).post_concat(transform);
            for points in [
                &[(23.0, 31.0), (31.0, 22.0), (39.0, 31.0)][..],
                &[(23.0, 49.0), (31.0, 58.0), (39.0, 49.0)][..],
            ] {
                if let Some(path) = line_path(points) {
                    draw_glowing_path(pm, &path, t, alpha * (0.42 + wave * 0.58), 4.0, fill_rgba);
                }
            }
        }
        CursorAction::Text => {
            let opacity = if reduced || progress < 0.34 || progress > 0.64 {
                alpha
            } else {
                alpha * 0.18
            };
            let t = Transform::from_translate(-4.0, 0.0).post_concat(transform);
            for points in [
                &[(31.0, 22.0), (31.0, 58.0)][..],
                &[(24.0, 22.0), (38.0, 22.0)][..],
                &[(24.0, 58.0), (38.0, 58.0)][..],
            ] {
                if let Some(path) = line_path(points) {
                    draw_glowing_path(pm, &path, t, opacity, 4.0, fill_rgba);
                }
            }
        }
        CursorAction::Key => {
            let bounce = if reduced {
                0.0
            } else {
                (progress * std::f32::consts::TAU).sin() * (1.0 - progress) * 3.0
            };
            let t = Transform::from_translate(-9.0, bounce).post_concat(transform);
            if let Some(rect) = tiny_skia::Rect::from_xywh(14.0, 25.0, 28.0, 28.0) {
                if let Some(path) =
                    rounded_rect_path(rect.x(), rect.y(), rect.width(), rect.height(), 6.0)
                {
                    draw_glowing_path(pm, &path, t, alpha, 3.5, fill_rgba);
                }
            }
            if let Some(path) = line_path(&[
                (23.0, 32.0),
                (23.0, 46.0),
                (23.0, 39.0),
                (33.0, 32.0),
                (24.0, 39.0),
                (34.0, 46.0),
            ]) {
                draw_glowing_path(pm, &path, t, alpha, 3.5, fill_rgba);
            }
        }
        CursorAction::Navigate => {
            let x = if reduced {
                0.0
            } else {
                -5.0 + ease_in_out(progress) * 9.0
            };
            let t = Transform::from_translate(x - 5.0, 0.0).post_concat(transform);
            for points in [
                &[(15.0, 29.0), (25.0, 40.0), (15.0, 51.0)][..],
                &[(29.0, 29.0), (39.0, 40.0), (29.0, 51.0)][..],
            ] {
                if let Some(path) = line_path(points) {
                    draw_glowing_path(pm, &path, t, alpha * (0.2 + wave * 0.8), 4.0, fill_rgba);
                }
            }
        }
        CursorAction::App => {
            let s = if reduced {
                1.0
            } else {
                0.2 + ease_in_out((progress * 2.0).min(1.0)) * 0.8
            };
            let t = Transform::from_translate(-26.0, -39.0)
                .post_scale(s, s)
                .post_translate(26.0 - 5.0, 39.0)
                .post_concat(transform);
            for (x, y) in [(13.0, 26.0), (29.0, 26.0), (13.0, 42.0), (29.0, 42.0)] {
                if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, 10.0, 10.0) {
                    if let Some(path) =
                        rounded_rect_path(rect.x(), rect.y(), rect.width(), rect.height(), 2.0)
                    {
                        draw_glowing_path(pm, &path, t, alpha, 3.5, fill_rgba);
                    }
                }
            }
        }
        CursorAction::Transfer => {
            let y = if reduced { 0.0 } else { 6.0 - wave * 12.0 };
            let t = Transform::from_translate(-9.0, y).post_concat(transform);
            for points in [
                &[
                    (22.0, 50.0),
                    (22.0, 20.0),
                    (14.0, 28.0),
                    (22.0, 20.0),
                    (30.0, 28.0),
                ][..],
                &[
                    (37.0, 28.0),
                    (37.0, 58.0),
                    (29.0, 50.0),
                    (37.0, 58.0),
                    (45.0, 50.0),
                ][..],
            ] {
                if let Some(path) = line_path(points) {
                    draw_glowing_path(pm, &path, t, alpha * (0.38 + wave * 0.62), 4.0, fill_rgba);
                }
            }
        }
        CursorAction::Record => {
            let t = Transform::from_translate(-12.0, 0.0).post_concat(transform);
            let mut ring = PathBuilder::new();
            ring.push_circle(29.0, 39.0, 17.0);
            if let Some(path) = ring.finish() {
                draw_glowing_path(pm, &path, t, alpha, 4.0, fill_rgba);
            }
            let radius = if reduced { 5.0 } else { 3.6 + wave * 2.1 };
            let mut dot = PathBuilder::new();
            dot.push_circle(29.0, 39.0, radius);
            if let Some(path) = dot.finish() {
                draw_cue_glow(
                    pm,
                    &path,
                    t,
                    alpha * (0.42 + wave * 0.58),
                    radius * 1.25,
                    fill_rgba,
                );
                let (outline_paint, outline_stroke) = cue_stroke(alpha * (0.42 + wave * 0.58), 3.0);
                pm.stroke_path(&path, &outline_paint, &outline_stroke, t, None);
                pm.fill_path(
                    &path,
                    &solid_paint(
                        Color::from_rgba8(fill_rgba[0], fill_rgba[1], fill_rgba[2], fill_rgba[3]),
                        alpha * (0.42 + wave * 0.58),
                    ),
                    FillRule::Winding,
                    t,
                    None,
                );
            }
        }
        CursorAction::System => {
            let rotation = if reduced {
                0.0
            } else {
                ease_in_out(progress) * 68.0 - 18.0
            };
            let t = Transform::from_translate(-29.0, -39.0)
                .post_rotate(rotation)
                .post_translate(29.0 - 14.0, 39.0)
                .post_concat(transform);
            for radius in [12.0, 4.0] {
                let mut circle = PathBuilder::new();
                circle.push_circle(29.0, 39.0, radius);
                if let Some(path) = circle.finish() {
                    draw_glowing_path(pm, &path, t, alpha, 3.5, fill_rgba);
                }
            }
            for points in [
                &[(29.0, 20.0), (29.0, 25.0)][..],
                &[(29.0, 53.0), (29.0, 58.0)][..],
                &[(10.0, 39.0), (15.0, 39.0)][..],
                &[(43.0, 39.0), (48.0, 39.0)][..],
                &[(16.0, 26.0), (20.0, 30.0)][..],
                &[(38.0, 48.0), (42.0, 52.0)][..],
                &[(16.0, 52.0), (20.0, 48.0)][..],
                &[(38.0, 30.0), (42.0, 26.0)][..],
            ] {
                if let Some(path) = line_path(points) {
                    draw_glowing_path(pm, &path, t, alpha, 3.5, fill_rgba);
                }
            }
        }
    }
}

fn draw_modifiers(
    pm: &mut tiny_skia::Pixmap,
    visual: &CursorVisualState,
    transform: Transform,
    alpha: f32,
    fill_rgba: [u8; 4],
) {
    if visual.delivery == Some(DeliveryModifier::Background) {
        let mut builder = PathBuilder::new();
        builder.move_to(34.0, 23.0);
        builder.cubic_to(20.0, 31.0, 17.0, 48.0, 21.0, 63.0);
        builder.cubic_to(25.0, 80.0, 38.0, 95.0, 53.0, 106.0);
        if let Some(path) = builder.finish() {
            draw_cue_glow(pm, &path, transform, alpha * 0.75, 2.5, fill_rgba);
            let dash = tiny_skia::StrokeDash::new(vec![1.0, 5.0], 0.0).unwrap();
            let (paint, mut outline_stroke) = cue_stroke(alpha * 0.75, 4.0);
            outline_stroke.dash = Some(dash.clone());
            pm.stroke_path(&path, &paint, &outline_stroke, transform, None);
            let mut core_stroke = Stroke {
                width: 1.5,
                line_cap: tiny_skia::LineCap::Round,
                line_join: tiny_skia::LineJoin::Round,
                ..Default::default()
            };
            core_stroke.dash = Some(dash);
            let color = Color::from_rgba8(fill_rgba[0], fill_rgba[1], fill_rgba[2], fill_rgba[3]);
            pm.stroke_path(
                &path,
                &solid_paint(color, alpha * 0.75),
                &core_stroke,
                transform,
                None,
            );
        }
    }

    if visual.delivery == Some(DeliveryModifier::Foreground) {
        let mut ring = PathBuilder::new();
        ring.push_circle(104.0, 96.0, 9.0);
        if let Some(path) = ring.finish() {
            draw_glowing_path(pm, &path, transform, alpha, 3.0, fill_rgba);
        }
    }

    match visual.target {
        Some(TargetModifier::Ax) => {
            for (x, y) in [(104.0, 89.0), (94.0, 104.0), (114.0, 104.0)] {
                let mut circle = PathBuilder::new();
                circle.push_circle(x, y, 3.0);
                if let Some(path) = circle.finish() {
                    draw_glowing_path(pm, &path, transform, alpha, 2.5, fill_rgba);
                }
            }
            if let Some(path) = line_path(&[
                (104.0, 92.0),
                (104.0, 97.0),
                (94.0, 101.0),
                (104.0, 97.0),
                (114.0, 101.0),
            ]) {
                draw_glowing_path(pm, &path, transform, alpha, 2.5, fill_rgba);
            }
        }
        Some(TargetModifier::Pixel) => {
            if let Some(rect) = tiny_skia::Rect::from_xywh(94.0, 91.0, 19.0, 19.0) {
                let path = PathBuilder::from_rect(rect);
                draw_cue_glow(pm, &path, transform, alpha, 2.5, fill_rgba);
                let dash = tiny_skia::StrokeDash::new(vec![2.0, 3.0], 0.0).unwrap();
                let (paint, mut outline_stroke) = cue_stroke(alpha, 4.0);
                outline_stroke.dash = Some(dash.clone());
                pm.stroke_path(&path, &paint, &outline_stroke, transform, None);
                let mut core_stroke = Stroke {
                    width: 1.5,
                    line_cap: tiny_skia::LineCap::Round,
                    line_join: tiny_skia::LineJoin::Round,
                    ..Default::default()
                };
                core_stroke.dash = Some(dash);
                let color =
                    Color::from_rgba8(fill_rgba[0], fill_rgba[1], fill_rgba[2], fill_rgba[3]);
                pm.stroke_path(
                    &path,
                    &solid_paint(color, alpha),
                    &core_stroke,
                    transform,
                    None,
                );
            }
        }
        Some(TargetModifier::Browser) => {
            let mut circle = PathBuilder::new();
            circle.push_circle(104.0, 100.0, 10.0);
            if let Some(path) = circle.finish() {
                draw_glowing_path(pm, &path, transform, alpha, 2.5, fill_rgba);
            }
            for points in [
                &[(94.0, 100.0), (114.0, 100.0)][..],
                &[(104.0, 90.0), (100.0, 100.0), (104.0, 110.0)][..],
                &[(104.0, 90.0), (108.0, 100.0), (104.0, 110.0)][..],
            ] {
                if let Some(path) = line_path(points) {
                    draw_glowing_path(pm, &path, transform, alpha, 2.5, fill_rgba);
                }
            }
        }
        Some(TargetModifier::Desktop) => {
            if let Some(rect) = tiny_skia::Rect::from_xywh(93.0, 90.0, 21.0, 15.0) {
                if let Some(path) =
                    rounded_rect_path(rect.x(), rect.y(), rect.width(), rect.height(), 2.0)
                {
                    draw_glowing_path(pm, &path, transform, alpha, 2.5, fill_rgba);
                }
            }
            if let Some(path) = line_path(&[
                (103.5, 105.0),
                (103.5, 110.0),
                (97.0, 110.0),
                (110.0, 110.0),
            ]) {
                draw_glowing_path(pm, &path, transform, alpha, 2.5, fill_rgba);
            }
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_profile_has_twelve_unique_actions() {
        let mut names = std::collections::BTreeSet::new();
        for action in CursorAction::ALL {
            assert!(names.insert(action.as_str()));
        }
        assert_eq!(names.len(), 12);
    }

    #[test]
    fn default_theme_uses_compact_48_point_footprint() {
        assert_eq!(DISPLAY_SIZE, 48.0);
    }

    #[test]
    fn default_cursor_keeps_original_blue_fill() {
        assert_eq!(session_fill_rgba("default"), DEFAULT_CURSOR_FILL);
        assert_eq!(session_fill_hex("default"), "#5EC0E8");
    }

    #[test]
    fn named_session_colors_are_stable_and_distinct() {
        assert_eq!(session_fill_rgba("agent-1"), session_fill_rgba("agent-1"));
        assert_ne!(session_fill_rgba("agent-1"), session_fill_rgba("agent-2"));
        assert_ne!(session_fill_rgba("agent-2"), DEFAULT_CURSOR_FILL);
    }

    #[test]
    fn default_cursor_uses_parameterized_fill_white_ink_and_matching_glow() {
        let mut pixmap = tiny_skia::Pixmap::new(256, 256).unwrap();
        paint_default_theme_with_fill(
            &mut pixmap,
            &CursorVisualState::default(),
            128.0,
            128.0,
            std::f32::consts::FRAC_PI_4,
            2.0,
            1.0,
            [12, 34, 56, 255],
        );

        let pixels = pixmap.data().chunks_exact(4).collect::<Vec<_>>();
        assert!(pixels.iter().any(|pixel| *pixel == [12, 34, 56, 255]));
        assert!(pixels.iter().any(|pixel| *pixel == [255, 255, 255, 255]));
        assert!(pixels.iter().any(|pixel| pixel[2] > pixel[1]
            && pixel[1] > pixel[0]
            && pixel[0] > 0
            && pixel[3] > 16
            && pixel[3] < 200));
    }

    #[test]
    fn matching_glow_surrounds_the_full_pointer_silhouette() {
        let mut pixmap = tiny_skia::Pixmap::new(256, 256).unwrap();
        let visual = CursorVisualState {
            reduced_motion: ReducedMotion::On,
            ..CursorVisualState::default()
        };
        paint_default_theme_with_fill(
            &mut pixmap,
            &visual,
            128.0,
            128.0,
            std::f32::consts::FRAC_PI_4,
            2.0,
            1.0,
            [12, 34, 56, 255],
        );

        let bounds = |predicate: &dyn Fn(&[u8]) -> bool| {
            let mut result = (u32::MAX, u32::MAX, 0, 0);
            for (index, pixel) in pixmap.data().chunks_exact(4).enumerate() {
                if predicate(pixel) {
                    let x = index as u32 % pixmap.width();
                    let y = index as u32 / pixmap.width();
                    result.0 = result.0.min(x);
                    result.1 = result.1.min(y);
                    result.2 = result.2.max(x);
                    result.3 = result.3.max(y);
                }
            }
            result
        };
        let body = bounds(&|pixel| pixel == [12, 34, 56, 255]);
        let glow = bounds(&|pixel| {
            pixel[2] > pixel[1]
                && pixel[1] > pixel[0]
                && pixel[0] > 0
                && pixel[3] > 8
                && pixel[3] < 220
        });

        assert!(
            glow.0 + 4 <= body.0,
            "glow should cover the pointer's left edge: body={body:?}, glow={glow:?}"
        );
        assert!(
            glow.1 + 4 <= body.1,
            "glow should cover the pointer's top edge: body={body:?}, glow={glow:?}"
        );
        assert!(
            glow.2 >= body.2 + 4,
            "glow should cover the pointer's right edge: body={body:?}, glow={glow:?}"
        );
        assert!(
            glow.3 >= body.3 + 4,
            "glow should cover the pointer's bottom edge: body={body:?}, glow={glow:?}"
        );
    }

    #[test]
    fn semantic_marks_use_a_tighter_softer_session_glow() {
        let mut pixmap = tiny_skia::Pixmap::new(96, 96).unwrap();
        let path = line_path(&[(30.0, 48.0), (66.0, 48.0)]).unwrap();
        draw_glowing_path(
            &mut pixmap,
            &path,
            Transform::identity(),
            1.0,
            4.0,
            [12, 34, 56, 255],
        );

        let pixel = |x: u32, y: u32| {
            let index = ((y * pixmap.width() + x) * 4) as usize;
            &pixmap.data()[index..index + 4]
        };
        assert_eq!(pixel(48, 48), [12, 34, 56, 255]);
        assert!(
            pixmap
                .data()
                .chunks_exact(4)
                .any(|sample| sample == [255, 255, 255, 255]),
            "semantic mark should retain a white outline"
        );
        let glow_pixel = pixel(48, 41);
        assert!(
            glow_pixel[2] > glow_pixel[0] && glow_pixel[3] > 0 && glow_pixel[3] < 180,
            "unexpected glow pixel: {glow_pixel:?}"
        );
        assert_eq!(pixel(48, 30), [0, 0, 0, 0]);
    }

    #[test]
    fn one_shot_returns_to_idle() {
        let mut state = CursorVisualState::default();
        state.begin(CursorAction::Click, None, Some(TargetModifier::Pixel));
        state.tick(CursorAction::Click.duration_secs() + 0.01);
        assert_eq!(state.resolved_action, CursorAction::Idle);
        assert_eq!(state.target, None);
    }

    #[test]
    fn held_action_waits_for_matching_end() {
        let mut state = CursorVisualState::default();
        state.begin(CursorAction::Text, Some(DeliveryModifier::Foreground), None);
        state.tick(30.0);
        assert_eq!(state.resolved_action, CursorAction::Text);
        state.end(CursorAction::Click);
        assert_eq!(state.resolved_action, CursorAction::Text);
        state.end(CursorAction::Text);
        assert_eq!(state.resolved_action, CursorAction::Text);
        state.tick(0.41);
        assert_eq!(state.resolved_action, CursorAction::Idle);
    }

    #[test]
    fn every_action_renders_pixels_at_one_and_two_x() {
        for action in CursorAction::ALL {
            for scale in [1.0, 2.0] {
                let mut pm = tiny_skia::Pixmap::new(256, 256).unwrap();
                let mut visual = CursorVisualState::default();
                visual.begin(
                    action,
                    Some(DeliveryModifier::Background),
                    Some(TargetModifier::Ax),
                );
                visual.elapsed_secs = action.duration_secs() * 0.4;
                paint_default_theme(&mut pm, &visual, 128.0, 128.0, 0.0, scale, 1.0);
                assert!(
                    pm.data().chunks_exact(4).any(|pixel| pixel[3] != 0),
                    "{} did not render at {scale}x",
                    action.as_str()
                );
            }
        }
    }
}
