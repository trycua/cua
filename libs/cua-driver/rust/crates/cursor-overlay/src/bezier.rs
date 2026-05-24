//! Cubic Bezier math — 1:1 port of `Bezier.swift` and
//! `CursorMotionSegment` from `AgentCursorPathPlanner.cs`.
//!
//! `B(t) = (1-t)³P₀ + 3(1-t)²tP₁ + 3(1-t)t²P₂ + t³P₃`

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };
    pub fn new(x: f64, y: f64) -> Self { Self { x, y } }
    pub fn hypot(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx.hypot(dy)
    }
}

/// Cubic Bezier segment with cached arc-length.
#[derive(Debug, Clone, Copy)]
pub struct CubicBezier {
    pub start: Point,
    pub control1: Point,
    pub control2: Point,
    pub end: Point,
    /// Arc-length via 32-point numerical integration.
    pub length: f64,
}

impl CubicBezier {
    pub fn new(start: Point, control1: Point, control2: Point, end: Point) -> Self {
        let mut s = Self { start, control1, control2, end, length: 0.0 };
        s.length = s.measure_length();
        s
    }

    /// Point on the curve at `t ∈ [0,1]`.
    pub fn point_at(&self, t: f64) -> Point {
        let t = t.clamp(0.0, 1.0);
        let u = 1.0 - t;
        let (uu, uuu) = (u * u, u * u * u);
        let (tt, ttt) = (t * t, t * t * t);
        Point {
            x: uuu * self.start.x
                + 3.0 * uu * t  * self.control1.x
                + 3.0 * u  * tt * self.control2.x
                + ttt * self.end.x,
            y: uuu * self.start.y
                + 3.0 * uu * t  * self.control1.y
                + 3.0 * u  * tt * self.control2.y
                + ttt * self.end.y,
        }
    }

    /// Tangent direction at `t`. Returns a unit-like vector (not normalised).
    pub fn tangent_at(&self, t: f64) -> Point {
        let t = t.clamp(0.0, 1.0);
        let u = 1.0 - t;
        let a = 3.0 * u * u;
        let b = 6.0 * u * t;
        let c = 3.0 * t * t;
        Point {
            x: a * (self.control1.x - self.start.x)
                + b * (self.control2.x - self.control1.x)
                + c * (self.end.x - self.control2.x),
            y: a * (self.control1.y - self.start.y)
                + b * (self.control2.y - self.control1.y)
                + c * (self.end.y - self.control2.y),
        }
    }

    /// Heading in radians at parametric `t`.
    pub fn heading_at(&self, t: f64) -> f64 {
        let tg = self.tangent_at(t);
        tg.y.atan2(tg.x)
    }

    fn measure_length(&self) -> f64 {
        let mut len = 0.0;
        let mut prev = self.start;
        for i in 1..=32 {
            let p = self.point_at(i as f64 / 32.0);
            len += prev.hypot(p);
            prev = p;
        }
        len
    }

    /// Sample at a given arc-distance from start (0..=self.length).
    /// Returns (point, heading_radians).
    pub fn sample_at_distance(&self, dist: f64) -> (Point, f64) {
        if self.length <= 0.0 {
            return (self.end, self.heading_at(1.0));
        }
        let t = (dist / self.length).clamp(0.0, 1.0);
        (self.point_at(t), self.heading_at(t))
    }
}

/// Build a `CubicBezier` from the `CursorMotionPath.Options` knobs.
/// Matches the Swift `CursorMotionPath.init(from:to:options:)` formula.
pub fn build_motion_bezier(
    start: Point,
    end: Point,
    start_handle: f64,
    end_handle: f64,
    arc_size: f64,
    arc_flow: f64,
) -> CubicBezier {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let length = dx.hypot(dy).max(1.0);

    // Unit perpendicular (90° CCW of direction vector).
    let perp_x = -dy / length;
    let perp_y =  dx / length;

    let deflection = length * arc_size;
    let flow_bias = (arc_flow + 1.0) / 2.0;  // map [-1,1] → [0,1]

    let c1_deflect = deflection * (1.0 - 0.5 * flow_bias);
    let c2_deflect = deflection * (1.0 - 0.5 * (1.0 - flow_bias));

    let c1_base = Point::new(start.x + dx * start_handle, start.y + dy * start_handle);
    let c2_base = Point::new(end.x   - dx * end_handle,   end.y   - dy * end_handle);

    let control1 = Point::new(c1_base.x + perp_x * c1_deflect, c1_base.y + perp_y * c1_deflect);
    let control2 = Point::new(c2_base.x + perp_x * c2_deflect, c2_base.y + perp_y * c2_deflect);

    CubicBezier::new(start, control1, control2, end)
}
