//! Path shapes with an arc-length table, so speed curves act on distance.

use crate::geom::{dist, lerp, lerp_pt, perp, unit, Pt};

/// A parametric path with an arc-length table. `at_fraction` extrapolates
/// along the end tangents outside `[0, 1]`, which is how overshoot leaves the
/// path and comes back.
pub struct Path {
    f: Box<dyn Fn(f64) -> Pt>,
    us: Vec<f64>,
    ss: Vec<f64>,
    pub length: f64,
    p0: Pt,
    p1: Pt,
    t0: Pt,
    t1: Pt,
}

impl Path {
    pub fn new(f: impl Fn(f64) -> Pt + 'static, n: usize) -> Self {
        let mut us = Vec::with_capacity(n + 1);
        let mut ss = Vec::with_capacity(n + 1);
        us.push(0.0);
        ss.push(0.0);
        let mut prev = f(0.0);
        let mut total = 0.0;
        for i in 1..=n {
            let u = i as f64 / n as f64;
            let p = f(u);
            total += (p.x - prev.x).hypot(p.y - prev.y);
            us.push(u);
            ss.push(total);
            prev = p;
        }
        let p0 = f(0.0);
        let p1 = f(1.0);
        let t0 = unit(f(1e-3), p0);
        let t1 = unit(f(1.0 - 1e-3), p1);
        Self {
            f: Box::new(f),
            us,
            ss,
            length: total,
            p0,
            p1,
            t0,
            t1,
        }
    }

    /// The point `frac` of the way along the path by distance.
    pub fn at_fraction(&self, frac: f64) -> Pt {
        let len = self.length;
        if len < 1e-9 {
            return (self.f)(frac.clamp(0.0, 1.0));
        }
        if frac > 1.0 {
            return Pt::new(
                self.p1.x + self.t1.x * (frac - 1.0) * len,
                self.p1.y + self.t1.y * (frac - 1.0) * len,
            );
        }
        if frac < 0.0 {
            return Pt::new(
                self.p0.x + self.t0.x * -frac * len,
                self.p0.y + self.t0.y * -frac * len,
            );
        }
        let target = frac * len;
        let (mut lo, mut hi) = (0usize, self.ss.len() - 1);
        while hi - lo > 1 {
            let mid = (lo + hi) >> 1;
            if self.ss[mid] < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let span = self.ss[hi] - self.ss[lo];
        let span = if span == 0.0 { 1.0 } else { span };
        let u = lerp(self.us[lo], self.us[hi], (target - self.ss[lo]) / span);
        (self.f)(u)
    }
}

fn cubic(p0: Pt, p1: Pt, p2: Pt, p3: Pt) -> impl Fn(f64) -> Pt {
    move |u| {
        let v = 1.0 - u;
        let a = v * v * v;
        let b = 3.0 * v * v * u;
        let c = 3.0 * v * u * u;
        let d = u * u * u;
        Pt::new(
            a * p0.x + b * p1.x + c * p2.x + d * p3.x,
            a * p0.y + b * p1.y + c * p2.y + d * p3.y,
        )
    }
}

fn quad(p0: Pt, p1: Pt, p2: Pt) -> impl Fn(f64) -> Pt {
    move |u| {
        let v = 1.0 - u;
        Pt::new(
            v * v * p0.x + 2.0 * v * u * p1.x + u * u * p2.x,
            v * v * p0.y + 2.0 * v * u * p1.y + u * u * p2.y,
        )
    }
}

/// Arc knobs of the Cua bezier.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ArcShape {
    /// First control point's distance from the start, as a fraction of the
    /// chord. [0, 1]
    pub start_handle: f64,
    /// Second control point's distance from the end. [0, 1]
    pub end_handle: f64,
    /// Sideways bend as a fraction of the chord; the sign picks the side.
    pub arc_size: f64,
    /// Where the bend peaks: -1 near the start, +1 near the end.
    pub arc_flow: f64,
}

/// The Cua bezier as a path.
pub fn cua_path(a: Pt, b: Pt, shape: ArcShape) -> Path {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len = dx.hypot(dy).max(1.0);
    let px = -dy / len;
    let py = dx / len;
    let deflection = len * shape.arc_size;
    let flow = (shape.arc_flow + 1.0) / 2.0;
    let c1d = deflection * (1.0 - 0.5 * flow);
    let c2d = deflection * (1.0 - 0.5 * (1.0 - flow));
    let c1 = Pt::new(
        a.x + dx * shape.start_handle + px * c1d,
        a.y + dy * shape.start_handle + py * c1d,
    );
    let c2 = Pt::new(
        b.x - dx * shape.end_handle + px * c2d,
        b.y - dy * shape.end_handle + py * c2d,
    );
    Path::new(cubic(a, c1, c2, b), 256)
}

/// Gentle single-sided quadratic curve (`paths.bow`).
pub fn bow_path(a: Pt, b: Pt, amount: f64) -> Path {
    let d = dist(a, b);
    let n = perp(unit(a, b));
    let m = lerp_pt(a, b, 0.5);
    let c = Pt::new(m.x + n.x * amount * d, m.y + n.y * amount * d);
    Path::new(quad(a, c, b), 256)
}

/// Straight line.
pub fn straight_path(a: Pt, b: Pt) -> Path {
    Path::new(move |u| lerp_pt(a, b, u), 8)
}

/// Chord side that bends paths upward for horizontal moves (`naturalSide`).
pub fn natural_side(a: Pt, b: Pt) -> f64 {
    if unit(a, b).x >= 0.0 {
        -1.0
    } else {
        1.0
    }
}
