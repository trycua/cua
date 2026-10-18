//! Geometry of the motion effects at a moment of a move: comet trail,
//! speed glow, magnet glow, click ripple and click squish. Renderers paint
//! these shapes in their own way (tiny-skia in Cua Driver, canvas on the
//! web) with the session colour lifted toward white ([`effect_rgb`]).

use crate::geom::POINTER_ANCHOR_OFFSET;
use crate::plan::Trajectory;
use crate::style::ResolvedEffects;

/// The comet trail.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct TrailSpec {
    /// How far back in time the trail reaches, seconds.
    pub secs: f64,
    /// Path length (points) at which the trail reaches full strength; a
    /// shorter trail (move start, landing) fades out instead of showing a
    /// stub.
    pub fade_len: f64,
    /// Segments.
    pub steps: usize,
    /// Stroke width at the tail and at the head, points.
    pub tail_width: f64,
    pub head_width: f64,
    /// Opacity at the head; it falls off with the square of the position.
    pub alpha: f64,
    /// How far behind the tip, along the arrow's axis, the trail is
    /// anchored, points. The arrow body covers the head of the trail, so the
    /// tip stays clean.
    pub anchor_offset: f64,
}

impl Default for TrailSpec {
    fn default() -> Self {
        Self {
            secs: 0.18,
            fade_len: 60.0,
            steps: 26,
            tail_width: 2.0,
            head_width: 12.0,
            alpha: 0.38,
            anchor_offset: POINTER_ANCHOR_OFFSET,
        }
    }
}

/// Magnet glow fade after lock-on, seconds.
pub const MAGNET_SECS: f64 = 0.7;
/// Magnet glow distance outside the target rect, points.
pub const MAGNET_INFLATE: f64 = 6.0;
/// Click ripple duration, seconds.
pub const RIPPLE_SECS: f64 = 0.52;
/// How long click effects (ripple, squish) keep the frame clock running.
pub const CLICK_FX_SECS: f64 = 0.55;
/// Click squish depth (fraction of the cursor size).
pub const SQUISH: f64 = 0.12;

/// Soft radial glow, offset against the velocity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glow {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub alpha: f64,
}

/// One round-capped stroke of the comet trail.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailSeg {
    pub a: (f64, f64),
    pub b: (f64, f64),
    pub width: f64,
    pub alpha: f64,
}

/// Glow around the target rect after a magnetic lock-on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Magnet {
    pub rect: [f64; 4],
    /// Fades 1 -> 0.
    pub glow: f64,
}

/// Expanding click ring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ripple {
    pub x: f64,
    pub y: f64,
    pub r: f64,
    pub width: f64,
    pub alpha: f64,
}

/// Everything to paint for one frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectFrame {
    pub glow: Option<Glow>,
    pub trail: Vec<TrailSeg>,
    pub magnet: Option<Magnet>,
    pub ripple: Option<Ripple>,
    /// Scale-down of the cursor artwork, 0 = none.
    pub squish: f64,
}

/// Anchor (body point) behind a sample's hotspot along its heading.
fn anchor(x: f64, y: f64, heading: f64, offset: f64) -> (f64, f64) {
    (x + heading.cos() * offset, y + heading.sin() * offset)
}

/// Speed glow at `t`, around `pos` (the cursor's anchor). `None` when the
/// cursor is slow or the move has finished.
pub fn glow(traj: &Trajectory, t: f64, pos: (f64, f64)) -> Option<Glow> {
    if t >= traj.duration() {
        return None;
    }
    let (vx, vy) = traj.velocity_at(t);
    let speed = vx.hypot(vy);
    let alpha = (speed * 0.00014).min(0.42);
    if alpha <= 0.02 {
        return None;
    }
    let off = (speed * 0.009).min(18.0);
    let (ux, uy) = (vx / speed, vy / speed);
    Some(Glow {
        x: pos.0 - ux * off,
        y: pos.1 - uy * off,
        r: 30.0 * (1.0 + (speed * 0.00024).min(0.44)),
        alpha,
    })
}

/// Comet trail at `t`, tail first. It follows the arrow's body (the
/// anchor), not the hotspot, so it flows out from behind the arrow.
pub fn trail(traj: &Trajectory, t: f64) -> Vec<TrailSeg> {
    let spec = traj.trail;
    let steps = spec.steps.max(1);
    let anchor_at = |k: f64| {
        let s = traj.sample_at(t - spec.secs + spec.secs * k);
        anchor(s.x, s.y, s.heading, spec.anchor_offset)
    };
    let pts: Vec<(f64, f64)> = (0..=steps)
        .map(|i| anchor_at(i as f64 / steps as f64))
        .collect();
    let length: f64 = pts
        .windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .sum();
    let fade = (length / spec.fade_len).min(1.0);
    let mut out = Vec::with_capacity(steps);
    for (i, w) in pts.windows(2).enumerate() {
        let k = (i + 1) as f64 / steps as f64;
        if (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1) > 0.3 {
            out.push(TrailSeg {
                a: w[0],
                b: w[1],
                width: spec.tail_width + (spec.head_width - spec.tail_width) * k,
                alpha: spec.alpha * k * k * fade,
            });
        }
    }
    out
}

/// Magnet glow at `t`, for a move with a lock-on time.
pub fn magnet(traj: &Trajectory, t: f64) -> Option<Magnet> {
    let snap = traj.snap_t?;
    let age = t - snap;
    if !(0.0..MAGNET_SECS).contains(&age) {
        return None;
    }
    let rect = if traj.target_known {
        traj.target
    } else {
        let end = traj.end();
        [end.x - 12.0, end.y - 12.0, 24.0, 24.0]
    };
    Some(Magnet {
        rect,
        glow: 1.0 - age / MAGNET_SECS,
    })
}

/// Click ripple `age` seconds after a click at `point`.
pub fn ripple(age: f64, point: (f64, f64)) -> Option<Ripple> {
    if age >= RIPPLE_SECS {
        return None;
    }
    let k = age / RIPPLE_SECS;
    let ease_out = 1.0 - (1.0 - k).powi(3);
    Some(Ripple {
        x: point.0,
        y: point.1,
        r: 8.0 + 44.0 * ease_out,
        width: 4.0 * (1.0 - k) + 1.0,
        alpha: 0.75 * (1.0 - k),
    })
}

/// Click squish `age` seconds after a click: quick in while pressed,
/// springy out after the release.
pub fn squish(age: f64) -> f64 {
    const PRESS: f64 = 0.09;
    if age < PRESS {
        SQUISH * (age / 0.05).min(1.0)
    } else {
        let after = age - PRESS;
        SQUISH
            * ((after / 0.22).min(1.0) * std::f64::consts::PI * 1.5)
                .cos()
                .max(0.0)
            * (1.0 - after / 0.22).max(0.0)
    }
}

/// The move's effects at `t` (glow, trail, magnet). `pos` is the cursor's
/// anchor; `blend` is whether the surface can draw translucent effects (the
/// glow and trail need it).
pub fn motion_frame(traj: &Trajectory, t: f64, pos: (f64, f64), blend: bool) -> EffectFrame {
    let fx = traj.effects;
    EffectFrame {
        glow: (fx.glow && blend).then(|| glow(traj, t, pos)).flatten(),
        trail: if fx.trail && blend {
            trail(traj, t)
        } else {
            Vec::new()
        },
        magnet: fx.magnet.then(|| magnet(traj, t)).flatten(),
        ripple: None,
        squish: 0.0,
    }
}

/// Add the click effects `age` seconds after a click at `point`.
pub fn add_click(frame: &mut EffectFrame, effects: ResolvedEffects, age: f64, point: (f64, f64)) {
    if effects.ripple {
        frame.ripple = ripple(age, point);
    }
    if effects.squish {
        frame.squish = squish(age);
    }
}

/// When a finished trajectory can be dropped: after its last sample, its
/// trail has caught up, and its magnet glow has faded.
pub fn linger(traj: &Trajectory) -> f64 {
    let mut end = traj.duration();
    if traj.effects.trail {
        end += traj.trail.secs;
    }
    if let (true, Some(snap)) = (traj.effects.magnet, traj.snap_t) {
        end = end.max(snap + MAGNET_SECS);
    }
    end
}

/// Effect colour: the cursor fill lifted toward white.
pub fn effect_rgb(fill: [u8; 3]) -> [u8; 3] {
    let lift = |c: u8| (f64::from(c) + (255.0 - f64::from(c)) * 0.45).round() as u8;
    [lift(fill[0]), lift(fill[1]), lift(fill[2])]
}
