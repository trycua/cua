// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Paint-time pose of the agent cursor: the float (`cursor.float`), the
//! rotation while it travels (`cursor.rotate`) and the halo (`cursor.halo`).
//!
//! All of it is visual only. The planner, its golden fixtures and the
//! hotspot are untouched: the float settles to exactly zero and the rotation
//! to upright by the time the hotspot arrives, so clicks land where they did.
//! Every motion the overlay plays (moves, click approaches, drags, the
//! magnetic snap, scroll and typing focus moves) is one planned trajectory,
//! so these functions cover all of them.

use crate::cursor_options::RotateMode;
use crate::trajectory::{wrap_angle, Trajectory};
use std::f64::consts::TAU;

/// Theme canvas units per point of the painted cursor.
const CANVAS_PT: f64 = crate::theme::DISPLAY_SIZE as f64 / crate::theme::CANVAS_SIZE as f64;

/// One float loop. The float is the one the cursor had from July to
/// October 2026 (#2603): a loop that sways side to side, bobs up and down
/// and tilts with the bob, at rest and while travelling.
pub const FLOAT_SECS: f64 = 4.0;
/// Horizontal sway amplitude, in points (5 canvas units; 3.3 pt peak to peak).
pub const FLOAT_SWAY_PT: f64 = 5.0 * CANVAS_PT;
/// Vertical bob amplitude, in points (6 canvas units; 3.9 pt peak to peak).
pub const FLOAT_BOB_PT: f64 = 6.0 * CANVAS_PT;
/// The bob is centred this far above the hotspot (5 canvas units).
pub const FLOAT_LIFT_PT: f64 = 5.0 * CANVAS_PT;
/// Tilt amplitude, phased with the bob.
pub const FLOAT_TILT_RADIANS: f64 = 2.5 * std::f64::consts::PI / 180.0;

/// After the cursor settles (an arrival, or a click, press, scroll or
/// typing cue at rest) the float stays at exactly 0 this long, so the
/// action that follows lands on the hotspot ...
pub const FLOAT_HOLD_SECS: f64 = 0.5;
/// ... then eases back in over this long.
pub const FLOAT_EASE_IN_SECS: f64 = 0.8;
/// A cue at rest with no move before it eases the float out this fast.
pub const FLOAT_EASE_OUT_SECS: f64 = 0.2;
/// Hops shorter than this keep no float while travelling (whatever float
/// they start with still eases out to 0).
pub const FLOAT_MIN_HOP_PT: f64 = 30.0;
const FLOAT_RAMP_IN_SECS: f64 = 0.25;
const FLOAT_RAMP_OUT_SECS: f64 = 0.35;

/// A float offset: `dx`, `dy` in points (y down, negative is up) and `tilt`
/// in radians (clockwise, y down), added to the heading rotation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FloatOffset {
    pub dx: f64,
    pub dy: f64,
    pub tilt: f64,
}

impl FloatOffset {
    pub const ZERO: Self = Self {
        dx: 0.0,
        dy: 0.0,
        tilt: 0.0,
    };

    /// This offset at `amplitude` (0..1). Exactly [`Self::ZERO`] at 0.
    pub fn scaled(self, amplitude: f64) -> Self {
        if amplitude <= 0.0 {
            return Self::ZERO;
        }
        let k = amplitude.min(1.0);
        Self {
            dx: self.dx * k,
            dy: self.dy * k,
            tilt: self.tilt * k,
        }
    }
}

/// The full float `secs` into its clock: sway `sin`, bob and tilt `cos`.
/// The clock belongs to the cursor and runs on across every action, so the
/// phase never snaps back.
pub fn float_at(secs: f64) -> FloatOffset {
    let angle = secs.rem_euclid(FLOAT_SECS) / FLOAT_SECS * TAU;
    FloatOffset {
        dx: FLOAT_SWAY_PT * angle.sin(),
        dy: FLOAT_BOB_PT * angle.cos() - FLOAT_LIFT_PT,
        tilt: FLOAT_TILT_RADIANS * angle.cos(),
    }
}

/// Float amplitude (0..1) at rest, `hold` seconds after the cursor last
/// settled, with amplitude `from` at that moment: `from` eases out to 0,
/// holds at 0 for [`FLOAT_HOLD_SECS`], then eases back in to 1.
pub fn rest_amplitude(hold: f64, from: f64) -> f64 {
    let out = from.clamp(0.0, 1.0) * (1.0 - smoothstep(hold / FLOAT_EASE_OUT_SECS));
    let back = smoothstep((hold - FLOAT_HOLD_SECS) / FLOAT_EASE_IN_SECS);
    out.max(back)
}

/// Float amplitude (0..1) while travelling, `t` seconds into `traj`, from
/// `start` (the amplitude when the move began, so nothing jumps). Full
/// mid-move on hops of [`FLOAT_MIN_HOP_PT`] or more; exactly 0 at arrival.
pub fn travel_amplitude(traj: &Trajectory, t: f64, start: f64) -> f64 {
    let arrival = traj.arrival_t;
    if t >= arrival {
        return 0.0;
    }
    let (p_in, p_out) = ramps(t, arrival, FLOAT_RAMP_IN_SECS, FLOAT_RAMP_OUT_SECS);
    let carry = start.clamp(0.0, 1.0) * (1.0 - p_in) * p_out;
    let full = if hop_length(traj) < FLOAT_MIN_HOP_PT {
        0.0
    } else {
        1.0
    };
    (carry + full * p_in * p_out).min(1.0)
}

/// `lean` turns the arrow at most this far.
pub const LEAN_MAX_RADIANS: f64 = 30.0 * std::f64::consts::PI / 180.0;
/// ... and at most this far while a button is held (drags).
pub const LEAN_MAX_DRAG_RADIANS: f64 = 15.0 * std::f64::consts::PI / 180.0;
/// Hops shorter than this never rotate.
pub const ROTATE_MIN_HOP_PT: f64 = 40.0;
const ROTATE_RAMP_IN_SECS: f64 = 0.15;
const ROTATE_RAMP_OUT_SECS: f64 = 0.25;

/// Centre of the upright `cua.default` arrow body, in points from the
/// hotspot (x right, y down).
pub const BODY_OFFSET_PT: (f64, f64) = (2.0, 11.9);

/// Direction from the arrow body to its tip (the hotspot) for the upright
/// arrow, in radians, y down.
pub fn tip_axis() -> f64 {
    (-BODY_OFFSET_PT.1).atan2(-BODY_OFFSET_PT.0)
}

/// Halo diameter is twice this, in points.
pub const HALO_RADIUS_PT: f32 = 14.0;
/// Halo opacity at its centre; it fades to 0 at the edge.
pub const HALO_PEAK_ALPHA: f32 = 0.11;
/// Halo gradient stops: (offset from the centre 0..1, share of the peak).
pub const HALO_STOPS: [(f32, f32); 3] = [(0.0, 1.0), (0.45, 0.55), (1.0, 0.0)];

fn smoothstep(x: f64) -> f64 {
    let t = x.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Ease-in after the start and ease-out before arrival, each at most half
/// the travel so short moves still reach exactly 0 at both ends.
fn ramps(t: f64, arrival: f64, ramp_in: f64, ramp_out: f64) -> (f64, f64) {
    let ramp_in = ramp_in.min(arrival * 0.5).max(1e-6);
    let ramp_out = ramp_out.min(arrival * 0.5).max(1e-6);
    (
        smoothstep(t / ramp_in),
        smoothstep((arrival - t) / ramp_out),
    )
}

/// Straight-line length of the move, hotspot start to end.
pub fn hop_length(traj: &Trajectory) -> f64 {
    let (a, b) = (traj.samples[0], traj.end());
    (b.x - a.x).hypot(b.y - a.y)
}

/// Whether the hotspot is still on its way (before arrival).
pub fn travelling(traj: &Trajectory, t: f64) -> bool {
    t < traj.arrival_t
}

/// Rotation in radians from upright, `t` seconds into `traj`. Upright at
/// the start, at arrival, after it, and on hops under
/// [`ROTATE_MIN_HOP_PT`]. `pressed` (a drag) caps `lean` at 15°.
pub fn travel_rotation(traj: &Trajectory, t: f64, mode: RotateMode, pressed: bool) -> f64 {
    if mode == RotateMode::Off || !travelling(traj, t) || hop_length(traj) < ROTATE_MIN_HOP_PT {
        return 0.0;
    }
    let (a, b) = (traj.samples[0], traj.end());
    let chord = (b.y - a.y).atan2(b.x - a.x);
    // `heading` can turn the arrow up to 180° (upside down on a move
    // straight down). Stretch its ramps with the size of the turn so a big
    // turn eases in and out as gently as a lean, with no flip.
    let stretch = match mode {
        RotateMode::Heading => 1.0 + wrap_angle(chord - tip_axis()).abs() / std::f64::consts::PI,
        _ => 1.0,
    };
    let (p_in, p_out) = ramps(
        t,
        traj.arrival_t,
        ROTATE_RAMP_IN_SECS * stretch,
        ROTATE_RAMP_OUT_SECS * stretch,
    );
    let (vx, vy) = traj.velocity_at(t);
    let travel = if vx.hypot(vy) > 1.0 {
        vy.atan2(vx)
    } else {
        chord
    };
    // Unwrap around the chord so a path that wobbles near straight down
    // never flips between +180° and -180°.
    let target = wrap_angle(chord - tip_axis()) + wrap_angle(travel - chord);
    let shaped = match mode {
        RotateMode::Lean => {
            let cap = if pressed {
                LEAN_MAX_DRAG_RADIANS
            } else {
                LEAN_MAX_RADIANS
            };
            cap * (target / cap).tanh()
        }
        _ => target,
    };
    shaped * p_in * p_out
}

/// Halo colour: the theme's accent with the same session tint the arrow
/// gets, so it follows theme and session changes.
pub fn halo_rgb(theme: Option<&crate::CompiledTheme>, cursor_id: &str) -> [u8; 3] {
    let session = crate::session_fill_rgba(cursor_id);
    let fallback;
    let theme = match theme {
        Some(theme) => theme,
        None => {
            fallback = crate::embedded_default_theme();
            &*fallback
        }
    };
    let tint = (theme.id == crate::DEFAULT_THEME_ID).then_some(session);
    let [r, g, b, _] = crate::theme_artifact::theme_accent_rgba(theme, tint).unwrap_or(session);
    [r, g, b]
}

/// Paint the halo: a radial glow of `radius` px around `(cx, cy)`, `peak`
/// opacity at the centre fading to 0 at the edge.
pub fn paint_halo(
    pm: &mut tiny_skia::Pixmap,
    cx: f32,
    cy: f32,
    radius: f32,
    rgb: [u8; 3],
    peak: f32,
) {
    use tiny_skia::{Color, GradientStop, PathBuilder, Point, RadialGradient, SpreadMode};
    let color = |a: f32| {
        Color::from_rgba8(
            rgb[0],
            rgb[1],
            rgb[2],
            (a.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    };
    if peak <= 0.0 || radius <= 0.0 {
        return;
    }
    let Some(shader) = RadialGradient::new(
        Point::from_xy(cx, cy),
        Point::from_xy(cx, cy),
        radius,
        HALO_STOPS
            .iter()
            .map(|&(offset, share)| GradientStop::new(offset, color(peak * share)))
            .collect(),
        SpreadMode::Pad,
        tiny_skia::Transform::identity(),
    ) else {
        return;
    };
    let paint = tiny_skia::Paint {
        shader,
        anti_alias: true,
        ..Default::default()
    };
    if let Some(circle) = PathBuilder::from_circle(cx, cy, radius) {
        pm.fill_path(
            &circle,
            &paint,
            tiny_skia::FillRule::Winding,
            tiny_skia::Transform::identity(),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trajectory::{plan_move, MoveRequest, Pt, REST_HEADING};
    use crate::MotionConfig;
    use std::f64::consts::PI;

    fn plan(from: (f64, f64), to: (f64, f64)) -> Trajectory {
        plan_move(
            &MotionConfig::default(),
            &MoveRequest {
                from: Pt::new(from.0, from.1),
                from_heading: REST_HEADING,
                to: Pt::new(to.0, to.1),
                end_heading: REST_HEADING,
                target: None,
                seed: "pose".into(),
                reduced_motion: false,
            },
        )
    }

    fn times(traj: &Trajectory) -> impl Iterator<Item = f64> + '_ {
        (0..=400).map(move |i| traj.arrival_t * f64::from(i) / 400.0)
    }

    #[test]
    fn the_float_is_the_historical_loop_within_its_bounds() {
        // #2603: sway 5, bob 6 about a lift of 5 canvas units, tilt 2.5°,
        // one loop every 4 s.
        let k = f64::from(crate::theme::DISPLAY_SIZE) / f64::from(crate::theme::CANVAS_SIZE);
        let at = |phase: f64| float_at(phase * FLOAT_SECS);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(at(0.0).dy, k) && close(at(0.0).dx, 0.0));
        assert!(close(at(0.0).tilt, 2.5f64.to_radians()));
        assert!(close(at(0.25).dx, 5.0 * k) && close(at(0.25).dy, -5.0 * k));
        assert!(close(at(0.5).dy, -11.0 * k) && close(at(0.5).tilt, -2.5f64.to_radians()));
        let (a, b) = (float_at(1.3), float_at(1.3 + FLOAT_SECS));
        assert!(
            close(a.dx, b.dx) && close(a.dy, b.dy) && close(a.tilt, b.tilt),
            "a 4 s loop"
        );
        let (mut dx, mut dy, mut tilt) = ((0f64, 0f64), (0f64, 0f64), (0f64, 0f64));
        for i in 0..=4000 {
            let f = float_at(FLOAT_SECS * f64::from(i) / 4000.0);
            dx = (dx.0.min(f.dx), dx.1.max(f.dx));
            dy = (dy.0.min(f.dy), dy.1.max(f.dy));
            tilt = (tilt.0.min(f.tilt), tilt.1.max(f.tilt));
        }
        // About 3.3 pt of sway, 3.9 pt of bob and 5° of tilt, peak to peak.
        assert!((dx.1 - dx.0 - 3.28).abs() < 0.01, "{dx:?}");
        assert!((dy.1 - dy.0 - 3.94).abs() < 0.01, "{dy:?}");
        assert!(
            (tilt.1 - tilt.0 - 5f64.to_radians()).abs() < 1e-6,
            "{tilt:?}"
        );
        // Never more than 3.61 pt from the hotspot (the old miss at a click).
        assert!(dy.0 >= -11.0 * k - 1e-9 && dy.1 <= k + 1e-9);
        // Any amplitude stays within the full float, and 0 is exactly 0.
        for amp in [0.0, 0.3, 0.7, 1.0, 1.4] {
            let f = float_at(2.1).scaled(amp);
            assert!(
                f.dx.abs() <= FLOAT_SWAY_PT + 1e-9 && f.tilt.abs() <= FLOAT_TILT_RADIANS + 1e-9
            );
            assert!(f.dy.abs() <= FLOAT_LIFT_PT + FLOAT_BOB_PT + 1e-9);
        }
        assert_eq!(float_at(2.1).scaled(0.0), FloatOffset::ZERO);
    }

    #[test]
    fn the_rest_amplitude_eases_out_holds_at_zero_and_eases_back_in() {
        assert_eq!(rest_amplitude(0.0, 0.0), 0.0);
        assert_eq!(rest_amplitude(0.0, 0.8), 0.8, "no jump when a cue arrives");
        assert_eq!(rest_amplitude(FLOAT_EASE_OUT_SECS, 1.0), 0.0);
        assert_eq!(rest_amplitude(FLOAT_HOLD_SECS, 1.0), 0.0, "held at 0");
        assert_eq!(
            rest_amplitude(FLOAT_HOLD_SECS + FLOAT_EASE_IN_SECS, 0.0),
            1.0
        );
        let mut last = rest_amplitude(0.0, 1.0);
        for i in 1..=600 {
            let a = rest_amplitude(f64::from(i) / 300.0, 1.0);
            assert!((0.0..=1.0).contains(&a));
            assert!((a - last).abs() < 0.05, "smooth at {i}: {last} -> {a}");
            last = a;
        }
    }

    #[test]
    fn the_travel_amplitude_carries_in_and_is_exactly_zero_at_arrival() {
        let traj = plan((100.0, 100.0), (700.0, 400.0));
        for start in [0.0, 0.4, 1.0] {
            assert_eq!(
                travel_amplitude(&traj, 0.0, start),
                start,
                "no jump at the start"
            );
            assert_eq!(travel_amplitude(&traj, traj.arrival_t, start), 0.0);
            assert!(travel_amplitude(&traj, traj.arrival_t - 1e-4, start) < 1e-3);
            assert_eq!(
                travel_amplitude(&traj, traj.arrival_t * 0.5, start),
                1.0,
                "floats in travel"
            );
            let mut last = start;
            for t in times(&traj) {
                let a = travel_amplitude(&traj, t, start);
                assert!((0.0..=1.0).contains(&a), "{a}");
                assert!((a - last).abs() < 0.1, "jump at {t}: {last} -> {a}");
                last = a;
            }
        }
    }

    #[test]
    fn short_hops_only_ease_the_float_out() {
        let traj = plan((100.0, 100.0), (120.0, 110.0));
        for t in times(&traj) {
            let a = travel_amplitude(&traj, t, 0.6);
            assert!((0.0..=0.6).contains(&a), "{a}");
        }
        assert_eq!(travel_amplitude(&traj, traj.arrival_t, 0.6), 0.0);
    }

    #[test]
    fn lean_is_capped_and_upright_at_both_ends() {
        for to in [
            (700.0, 400.0),
            (100.0, 600.0),
            (100.0, 20.0),
            (-400.0, 100.0),
        ] {
            let traj = plan((100.0, 100.0), to);
            let mut peak: f64 = 0.0;
            for t in times(&traj) {
                let r = travel_rotation(&traj, t, RotateMode::Lean, false);
                assert!(r.abs() <= LEAN_MAX_RADIANS + 1e-9, "{to:?} {r}");
                let dragging = travel_rotation(&traj, t, RotateMode::Lean, true);
                assert!(
                    dragging.abs() <= LEAN_MAX_DRAG_RADIANS + 1e-9,
                    "{to:?} {dragging}"
                );
                peak = peak.max(r.abs());
            }
            assert!(peak > 10f64.to_radians(), "{to:?} leans: {peak}");
            assert_eq!(travel_rotation(&traj, 0.0, RotateMode::Lean, false), 0.0);
            assert_eq!(
                travel_rotation(&traj, traj.arrival_t, RotateMode::Lean, false),
                0.0
            );
            assert_eq!(
                travel_rotation(&traj, traj.duration(), RotateMode::Heading, false),
                0.0
            );
        }
    }

    #[test]
    fn heading_points_the_tip_along_the_path() {
        let traj = plan((100.0, 300.0), (900.0, 300.0));
        let t = traj.arrival_t * 0.5;
        let (vx, vy) = traj.velocity_at(t);
        let travel = vy.atan2(vx);
        let r = travel_rotation(&traj, t, RotateMode::Heading, false);
        let tip = tip_axis() + r;
        assert!(
            wrap_angle(tip - travel).abs() < 0.05,
            "tip {tip} travel {travel}"
        );
        // A straight downward move turns the tip well past the 30° lean.
        let down = plan((300.0, 100.0), (300.0, 700.0));
        let r = travel_rotation(&down, down.arrival_t * 0.5, RotateMode::Heading, false);
        assert!(r.abs() > PI / 2.0, "{r}");
    }

    #[test]
    fn heading_turns_upside_down_on_downward_moves_without_a_snap() {
        // Straight down and a few degrees either side, where the angle
        // crosses +-180°: the turn stays continuous at 120 Hz.
        for to in [
            (300.0, 700.0),
            (290.0, 700.0),
            (310.0, 700.0),
            (240.0, 700.0),
        ] {
            let traj = plan((300.0, 100.0), to);
            let mut last = travel_rotation(&traj, 0.0, RotateMode::Heading, false);
            let mut peak: f64 = 0.0;
            let steps = (traj.arrival_t * 120.0).ceil() as usize;
            for i in 1..=steps {
                let t = traj.arrival_t * i as f64 / steps as f64;
                let r = travel_rotation(&traj, t, RotateMode::Heading, false);
                assert!(
                    (r - last).abs() < 10f64.to_radians(),
                    "{to:?}: jumped {:.1}° at {t:.3}",
                    (r - last).abs().to_degrees()
                );
                peak = peak.max(r.abs());
                last = r;
            }
            assert!(peak > 2.0 * PI / 3.0, "{to:?} turns over: {peak}");
            assert!(last.abs() < 1e-9, "{to:?} upright at arrival: {last}");
        }
    }

    #[test]
    fn short_hops_and_off_never_rotate() {
        let hop = plan((100.0, 100.0), (130.0, 110.0));
        let long = plan((100.0, 100.0), (700.0, 400.0));
        for t in times(&hop) {
            assert_eq!(travel_rotation(&hop, t, RotateMode::Heading, false), 0.0);
        }
        for t in times(&long) {
            assert_eq!(travel_rotation(&long, t, RotateMode::Off, false), 0.0);
        }
    }

    #[test]
    fn the_halo_takes_the_theme_accent_with_the_session_tint() {
        let default = crate::embedded_default_theme();
        let [r, g, b, _] = crate::DEFAULT_CURSOR_FILL;
        assert_eq!(halo_rgb(Some(&default), "default"), [r, g, b]);
        let [r, g, b, _] = crate::session_fill_rgba("agent-1");
        assert_eq!(halo_rgb(Some(&default), "agent-1"), [r, g, b]);
        assert_eq!(halo_rgb(None, "agent-1"), [r, g, b]);

        // A custom theme keeps its own accent, whatever the session.
        let mut custom = (*default).clone();
        custom.id = "example.orange".into();
        for animation in custom.actions.values_mut() {
            for frame in &mut animation.frames {
                for command in &mut frame.commands {
                    if command.fill == Some(crate::DEFAULT_CURSOR_FILL) {
                        command.fill = Some([250, 120, 20, 255]);
                    }
                }
            }
        }
        assert_eq!(halo_rgb(Some(&custom), "agent-1"), [250, 120, 20]);
    }

    #[test]
    fn the_halo_peaks_at_eleven_percent_and_fades_to_nothing_at_its_edge() {
        let mut pm = tiny_skia::Pixmap::new(64, 64).unwrap();
        paint_halo(&mut pm, 32.0, 32.0, 28.0, [94, 192, 232], HALO_PEAK_ALPHA);
        let alpha = |x: u32, y: u32| pm.pixel(x, y).unwrap().alpha();
        let centre = alpha(32, 32);
        assert!((25..=30).contains(&centre), "{centre}");
        assert!(alpha(32 + 20, 32) < centre / 2);
        assert!(alpha(32 + 27, 32) <= 2);
        assert_eq!(alpha(1, 1), 0);
    }

    #[test]
    fn the_body_offset_is_the_centre_of_the_painted_arrow() {
        let mut core = crate::RenderStateCore::new(crate::CursorConfig::default());
        core.options_override = Some(crate::CursorOptions::NONE);
        core.pos = crate::anchor_for_pointer(60.0, 40.0, REST_HEADING);
        core.placed = true;
        let mut pm = tiny_skia::Pixmap::new(120, 100).unwrap();
        crate::paint_cursor(&mut pm, &core, 0.0, 0.0, None, 1.0);
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for y in 0..100 {
            for x in 0..120 {
                if pm.pixel(x, y).unwrap().alpha() > 128 {
                    sx += f64::from(x) + 0.5;
                    sy += f64::from(y) + 0.5;
                    n += 1.0;
                }
            }
        }
        let (cx, cy) = (sx / n - 60.0, sy / n - 40.0);
        assert!((cx - BODY_OFFSET_PT.0).abs() < 0.5, "{cx}");
        assert!((cy - BODY_OFFSET_PT.1).abs() < 0.5, "{cy}");
        // The tip axis points up and a little left of the body.
        let axis = tip_axis().to_degrees();
        assert!((-105.0..-95.0).contains(&axis), "{axis}");
    }
}
