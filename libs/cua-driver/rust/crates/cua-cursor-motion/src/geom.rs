//! Points, angles and the cursor's tip-to-body offset.

use std::f64::consts::PI;

/// A point in screen space (points, y down).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

pub(crate) fn dist(a: Pt, b: Pt) -> f64 {
    (b.x - a.x).hypot(b.y - a.y)
}

pub(crate) fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

pub(crate) fn lerp_pt(a: Pt, b: Pt, t: f64) -> Pt {
    Pt::new(lerp(a.x, b.x, t), lerp(a.y, b.y, t))
}

pub(crate) fn unit(a: Pt, b: Pt) -> Pt {
    let d = dist(a, b);
    let d = if d == 0.0 { 1.0 } else { d };
    Pt::new((b.x - a.x) / d, (b.y - a.y) / d)
}

/// 90 degrees counter-clockwise in screen space (y down).
pub(crate) fn perp(u: Pt) -> Pt {
    Pt::new(-u.y, u.x)
}

/// JS `a % TAU` wrapped into `(-PI, PI]`, matching `math.js wrapAngle`.
pub fn wrap_angle(a: f64) -> f64 {
    let tau = 2.0 * PI;
    let mut r = a % tau;
    if r > PI {
        r -= tau;
    }
    if r < -PI {
        r += tau;
    }
    r
}

/// Distance from the cursor's hotspot (the tip) to its anchor (the body),
/// along the heading, in points. The trail flows from the anchor, and the
/// classic glide is planned in anchor space.
pub const POINTER_ANCHOR_OFFSET: f64 = 16.0;

/// Anchor that places a cursor's hotspot on `(x, y)` at `heading`.
pub fn anchor_for_pointer(x: f64, y: f64, heading: f64) -> (f64, f64) {
    (
        x + heading.cos() * POINTER_ANCHOR_OFFSET,
        y + heading.sin() * POINTER_ANCHOR_OFFSET,
    )
}

/// Pointer point, where the hotspot is drawn, for an anchor at `heading`.
pub fn pointer_for_anchor(x: f64, y: f64, heading: f64) -> (f64, f64) {
    (
        x - heading.cos() * POINTER_ANCHOR_OFFSET,
        y - heading.sin() * POINTER_ANCHOR_OFFSET,
    )
}
