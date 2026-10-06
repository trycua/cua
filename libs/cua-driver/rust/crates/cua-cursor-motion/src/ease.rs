//! Speed curves: how far along the path the cursor is at each moment.

use std::f64::consts::PI;

pub fn linear(t: f64) -> f64 {
    t
}
/// Minimum-jerk (Flash and Hogan): the profile of a relaxed human reach.
pub fn min_jerk(t: f64) -> f64 {
    t * t * t * (10.0 - 15.0 * t + 6.0 * t * t)
}
pub fn smootherstep(t: f64) -> f64 {
    t * t * t * (t * (6.0 * t - 15.0) + 10.0)
}
pub fn in_out_cubic(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}
pub fn in_out_sine(t: f64) -> f64 {
    0.5 - 0.5 * (PI * t).cos()
}
pub fn out_cubic(t: f64) -> f64 {
    1.0 - (1.0 - t).powi(3)
}

/// CSS-style `cubic-bezier(x1, y1, x2, y2)`: solve x(u) = t, return y(u).
pub fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let x1 = x1.clamp(0.0, 1.0);
    let x2 = x2.clamp(0.0, 1.0);
    let coord = |a: f64, b: f64, u: f64| {
        let v = 1.0 - u;
        3.0 * v * v * u * a + 3.0 * v * u * u * b + u * u * u
    };
    // Bisection: x(u) is monotonic for x1, x2 in [0, 1]. 52 halvings reach
    // f64 resolution and give the same answer in every language.
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..52 {
        let mid = 0.5 * (lo + hi);
        if coord(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    coord(y1, y2, 0.5 * (lo + hi))
}

/// A named speed curve, `progress = ease(time)` on `[0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case", tag = "type"))]
pub enum Ease {
    Linear,
    MinJerk,
    Smootherstep,
    InOutCubic,
    InOutSine,
    OutCubic,
    /// CSS `cubic-bezier()` control points.
    CubicBezier {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
}

impl Ease {
    pub fn apply(self, t: f64) -> f64 {
        match self {
            Self::Linear => linear(t),
            Self::MinJerk => min_jerk(t),
            Self::Smootherstep => smootherstep(t),
            Self::InOutCubic => in_out_cubic(t),
            Self::InOutSine => in_out_sine(t),
            Self::OutCubic => out_cubic(t),
            Self::CubicBezier { x1, y1, x2, y2 } => cubic_bezier(x1, y1, x2, y2, t),
        }
    }
}

/// `bumpProfile`: base plus a smooth bump peaking at `over_at` that pushes
/// `over` (fraction of the distance) past the end, then settles back.
pub fn bump_profile(base: impl Fn(f64) -> f64, over: f64, over_at: f64) -> impl Fn(f64) -> f64 {
    let bump = move |tau: f64, at: f64| {
        let a = (at * 10.0).max(1.5);
        let b = ((1.0 - at) * 10.0).max(1.5);
        let peak = (a / (a + b)).powf(a) * (b / (a + b)).powf(b);
        (tau.powf(a) * (1.0 - tau).powf(b)) / peak
    };
    move |tau| base(tau) + over * bump(tau, over_at)
}

/// `wobbleProfile`: damped wobble around the end from `start` on.
pub fn wobble_profile(
    base: impl Fn(f64) -> f64,
    amp: f64,
    cycles: f64,
    decay: f64,
    start: f64,
) -> impl Fn(f64) -> f64 {
    move |tau| {
        if tau <= start {
            return base(tau);
        }
        let u = (tau - start) / (1.0 - start);
        let ramp = smootherstep((u / 0.18).min(1.0));
        base(tau)
            + (amp * ramp * (-decay * u).exp() * (2.0 * PI * cycles * u).sin() * (1.0 - u).powi(2))
                / ((-decay * 0.12).exp() * 0.77).max(1e-6)
    }
}
