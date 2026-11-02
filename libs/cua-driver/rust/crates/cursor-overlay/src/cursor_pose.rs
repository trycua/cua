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

/// Peak lift of the resting float, in points (12 theme canvas units).
pub const REST_FLOAT_LIFT_PT: f64 =
    12.0 * crate::theme::DISPLAY_SIZE as f64 / crate::theme::CANVAS_SIZE as f64;
/// One resting float loop.
pub const REST_FLOAT_SECS: f64 = 4.0;
/// Lift held while the cursor travels.
pub const TRAVEL_LIFT_PT: f64 = 2.2;
/// Peak-to-peak bob on top of the travel lift.
pub const TRAVEL_BOB_PT: f64 = 0.8;
pub const TRAVEL_BOB_SECS: f64 = 0.9;
/// Hops shorter than this get no travel lift (the resting float still
/// eases out to zero).
pub const FLOAT_MIN_HOP_PT: f64 = 30.0;
const FLOAT_RAMP_IN_SECS: f64 = 0.18;
const FLOAT_RAMP_OUT_SECS: f64 = 0.24;

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

/// Resting float offset in points (negative is up) `secs` into the loop.
/// Starts and ends each loop at 0, so it picks up seamlessly on arrival.
pub fn resting_float_dy(secs: f64) -> f64 {
    let phase = secs.rem_euclid(REST_FLOAT_SECS) / REST_FLOAT_SECS;
    -0.5 * REST_FLOAT_LIFT_PT * (1.0 - (phase * TAU).cos())
}

/// Float offset in points while travelling, `t` seconds into `traj`.
/// `start_dy` is the offset the cursor had when the move began, so the
/// float carries over without a jump; it is exactly 0 at arrival.
pub fn travel_float_dy(traj: &Trajectory, t: f64, start_dy: f64) -> f64 {
    let arrival = traj.arrival_t;
    if t >= arrival {
        return 0.0;
    }
    let (p_in, p_out) = ramps(t, arrival, FLOAT_RAMP_IN_SECS, FLOAT_RAMP_OUT_SECS);
    let carry = start_dy * (1.0 - p_in);
    let lift = if hop_length(traj) < FLOAT_MIN_HOP_PT {
        0.0
    } else {
        TRAVEL_LIFT_PT + 0.5 * TRAVEL_BOB_PT * (t * TAU / TRAVEL_BOB_SECS).sin()
    };
    carry - lift * p_in * p_out
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
        vec![
            GradientStop::new(0.0, color(peak)),
            GradientStop::new(0.45, color(peak * 0.55)),
            GradientStop::new(1.0, color(0.0)),
        ],
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
    fn the_resting_float_starts_and_ends_each_loop_on_the_hotspot() {
        assert_eq!(resting_float_dy(0.0), 0.0);
        assert!((resting_float_dy(REST_FLOAT_SECS / 2.0) + REST_FLOAT_LIFT_PT).abs() < 1e-9);
        assert!(resting_float_dy(REST_FLOAT_SECS).abs() < 1e-9);
    }

    #[test]
    fn the_travel_float_is_zero_at_arrival_and_carries_the_resting_lift() {
        let traj = plan((100.0, 100.0), (700.0, 400.0));
        let start = -3.0;
        assert_eq!(
            travel_float_dy(&traj, 0.0, start),
            start,
            "no jump at the start"
        );
        assert_eq!(travel_float_dy(&traj, traj.arrival_t, start), 0.0);
        assert!(travel_float_dy(&traj, traj.arrival_t - 1e-4, start).abs() < 1e-3);
        let mid = travel_float_dy(&traj, traj.arrival_t * 0.5, start);
        assert!(
            (-(TRAVEL_LIFT_PT + TRAVEL_BOB_PT)..-(TRAVEL_LIFT_PT - TRAVEL_BOB_PT)).contains(&mid),
            "about 2.2 pt up mid-move: {mid}"
        );
        // Continuous: no frame-to-frame step bigger than a smooth ramp.
        let mut last = start;
        for t in times(&traj) {
            let dy = travel_float_dy(&traj, t, start);
            assert!((dy - last).abs() < 0.25, "jump at {t}: {last} -> {dy}");
            assert!(
                dy <= 0.0 && dy >= start.min(-TRAVEL_LIFT_PT - TRAVEL_BOB_PT),
                "{dy}"
            );
            last = dy;
        }
    }

    #[test]
    fn short_hops_only_ease_the_resting_lift_out() {
        let traj = plan((100.0, 100.0), (120.0, 110.0));
        for t in times(&traj) {
            let dy = travel_float_dy(&traj, t, -1.0);
            assert!((-1.0..=0.0).contains(&dy), "{dy}");
        }
        assert_eq!(travel_float_dy(&traj, traj.arrival_t, -1.0), 0.0);
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
