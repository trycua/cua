//! Dubins path planner — 1:1 port of the Swift `AgentCursorRenderer` Dubins
//! implementation (`planPath` / `planDubins` / `DubinsPlannedPath`).
//!
//! Plans a minimum-turning-radius arc–straight–arc path from `(x0,y0,th0)`
//! to `(x1,y1,th1)` with turn radius `R`, then samples it at any arc-length.

use std::f64::consts::PI;

// ── Public types ──────────────────────────────────────────────────────────

/// Position + heading at a sampled point on the path.
#[derive(Debug, Clone, Copy)]
pub struct PathState {
    pub x: f64,
    pub y: f64,
    pub heading: f64,
}

/// The kind of path (Dubins arc–straight–arc, or linear fallback).
#[derive(Debug, Clone, Copy, PartialEq)]
enum PathKind { Dubins, Linear }

/// The three Dubins segment types.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SegType { L, R, S }

/// A planned path (Dubins or linear fallback).
#[derive(Debug, Clone)]
pub struct PlannedPath {
    pub length: f64,
    pub end_visual_heading: f64,
    kind: PathKind,
    // Dubins state
    x0: f64, y0: f64, th0: f64,
    r: f64,
    seg1: f64, seg2: f64, seg3: f64,
    types: [SegType; 3],
    // Linear fallback
    x1: f64, y1: f64, th1: f64,
}

impl PlannedPath {
    /// Sample the path at `distance` arc-length from the start.
    /// Matches `DubinsPlannedPath.sample(at:)`.
    pub fn sample(&self, distance: f64) -> PathState {
        match self.kind {
            PathKind::Linear => self.sample_linear(distance),
            PathKind::Dubins => self.sample_dubins(distance),
        }
    }

    fn sample_linear(&self, s: f64) -> PathState {
        let len = self.length.max(1.0);
        let u = (s / len).clamp(0.0, 1.0);
        let mut diff = self.th1 - self.th0;
        while diff >  PI { diff -= 2.0 * PI; }
        while diff < -PI { diff += 2.0 * PI; }
        PathState {
            x:       self.x0 + (self.x1 - self.x0) * u,
            y:       self.y0 + (self.y1 - self.y0) * u,
            heading: self.th0 + diff * u,
        }
    }

    fn sample_dubins(&self, s_in: f64) -> PathState {
        if s_in <= 0.0 {
            return PathState { x: self.x0, y: self.y0, heading: self.th0 };
        }
        let r = self.r;
        let l1 = self.seg1 * r;
        let l2 = self.seg2 * r;
        let l3 = self.seg3 * r;
        let s = s_in.min(l1 + l2 + l3);

        let mut x  = self.x0;
        let mut y  = self.y0;
        let mut th = self.th0;

        let mut advance = |len: f64, seg: SegType| {
            if seg == SegType::S {
                x += th.cos() * len;
                y += th.sin() * len;
            } else {
                let dth = (len / r) * if seg == SegType::L { 1.0 } else { -1.0 };
                let perp = if seg == SegType::L { PI / 2.0 } else { -PI / 2.0 };
                let cx = x + (th + perp).cos() * r;
                let cy = y + (th + perp).sin() * r;
                let ang = (y - cy).atan2(x - cx);
                x = cx + (ang + dth).cos() * r;
                y = cy + (ang + dth).sin() * r;
                th += dth;
            }
        };

        if s <= l1 {
            advance(s, self.types[0]);
            return PathState { x, y, heading: th };
        }
        advance(l1, self.types[0]);
        if s <= l1 + l2 {
            advance(s - l1, self.types[1]);
            return PathState { x, y, heading: th };
        }
        advance(l2, self.types[1]);
        advance(s - l1 - l2, self.types[2]);
        PathState { x, y, heading: th }
    }
}

// ── PathPlanner entry-point ───────────────────────────────────────────────

pub struct PathPlanner;

impl PathPlanner {
    /// Plan a Dubins cursor path from `(x0,y0)` heading `th0` to `(x1,y1)` heading `th1`.
    ///
    /// * `end_visual_heading` — the arrow heading at rest (for the rendered tip).
    /// * `turn_radius` — minimum turning radius in points (Swift default: 80).
    pub fn plan(
        x0: f64, y0: f64, th0: f64,
        x1: f64, y1: f64, th1: f64,
        end_visual_heading: f64,
        turn_radius: f64,
    ) -> PlannedPath {
        let r = turn_radius.max(1.0);
        if let Some(p) = plan_dubins(x0, y0, th0, x1, y1, th1, r, end_visual_heading) {
            return p;
        }
        // Linear fallback.
        let d = (x1 - x0).hypot(y1 - y0).max(1.0);
        PlannedPath {
            length: d,
            end_visual_heading,
            kind: PathKind::Linear,
            x0, y0, th0, r,
            seg1: 0.0, seg2: 0.0, seg3: 0.0,
            types: [SegType::S, SegType::S, SegType::S],
            x1, y1, th1,
        }
    }
}

// ── Dubins internals ──────────────────────────────────────────────────────

fn mod2pi(x: f64) -> f64 {
    let tau = 2.0 * PI;
    let r = x - tau * (x / tau).floor();
    if r < 0.0 { r + tau } else { r }
}

struct DubinsSol {
    t: f64, p: f64, q: f64,
    types: [SegType; 3],
}

impl DubinsSol {
    fn length(&self) -> f64 { self.t + self.p + self.q }
}

fn dubins_lsl(d: f64, a: f64, b: f64) -> Option<DubinsSol> {
    let tmp0 = d + a.sin() - b.sin();
    let p2 = 2.0 + d * d - 2.0 * (a - b).cos() + 2.0 * d * (a.sin() - b.sin());
    if p2 < 0.0 { return None; }
    let tmp1 = (b.cos() - a.cos()).atan2(tmp0);
    Some(DubinsSol {
        t: mod2pi(-a + tmp1), p: p2.sqrt(), q: mod2pi(b - tmp1),
        types: [SegType::L, SegType::S, SegType::L],
    })
}

fn dubins_rsr(d: f64, a: f64, b: f64) -> Option<DubinsSol> {
    let tmp0 = d - a.sin() + b.sin();
    let p2 = 2.0 + d * d - 2.0 * (a - b).cos() + 2.0 * d * (b.sin() - a.sin());
    if p2 < 0.0 { return None; }
    let tmp1 = (a.cos() - b.cos()).atan2(tmp0);
    Some(DubinsSol {
        t: mod2pi(a - tmp1), p: p2.sqrt(), q: mod2pi(-b + tmp1),
        types: [SegType::R, SegType::S, SegType::R],
    })
}

fn dubins_lsr(d: f64, a: f64, b: f64) -> Option<DubinsSol> {
    let p2 = -2.0 + d * d + 2.0 * (a - b).cos() + 2.0 * d * (a.sin() + b.sin());
    if p2 < 0.0 { return None; }
    let p = p2.sqrt();
    let tmp1 = (-(a.cos() + b.cos())).atan2(d + a.sin() + b.sin()) - (-2.0_f64).atan2(p);
    Some(DubinsSol {
        t: mod2pi(-a + tmp1), p, q: mod2pi(-mod2pi(b) + tmp1),
        types: [SegType::L, SegType::S, SegType::R],
    })
}

fn dubins_rsl(d: f64, a: f64, b: f64) -> Option<DubinsSol> {
    let p2 = d * d - 2.0 + 2.0 * (a - b).cos() - 2.0 * d * (a.sin() + b.sin());
    if p2 < 0.0 { return None; }
    let p = p2.sqrt();
    let tmp1 = (a.cos() + b.cos()).atan2(d - a.sin() - b.sin()) - 2.0_f64.atan2(p);
    Some(DubinsSol {
        t: mod2pi(a - tmp1), p, q: mod2pi(b - tmp1),
        types: [SegType::R, SegType::S, SegType::L],
    })
}

fn dubins_rlr(d: f64, a: f64, b: f64) -> Option<DubinsSol> {
    let tmp = (6.0 - d * d + 2.0 * (a - b).cos() + 2.0 * d * (a.sin() - b.sin())) / 8.0;
    if tmp.abs() > 1.0 { return None; }
    let p = mod2pi(2.0 * PI - tmp.acos());
    let t = mod2pi(a - (a.cos() - b.cos()).atan2(d - a.sin() + b.sin()) + p / 2.0);
    Some(DubinsSol {
        t, p, q: mod2pi(a - b - t + p),
        types: [SegType::R, SegType::L, SegType::R],
    })
}

fn dubins_lrl(d: f64, a: f64, b: f64) -> Option<DubinsSol> {
    let tmp = (6.0 - d * d + 2.0 * (a - b).cos() + 2.0 * d * (b.sin() - a.sin())) / 8.0;
    if tmp.abs() > 1.0 { return None; }
    let p = mod2pi(2.0 * PI - tmp.acos());
    let t = mod2pi(-a + (-a.cos() + b.cos()).atan2(d + a.sin() - b.sin()) + p / 2.0);
    Some(DubinsSol {
        t, p, q: mod2pi(mod2pi(b) - a - t + p),
        types: [SegType::L, SegType::R, SegType::L],
    })
}

fn plan_dubins(
    x0: f64, y0: f64, th0: f64,
    x1: f64, y1: f64, th1: f64,
    r: f64,
    end_visual_heading: f64,
) -> Option<PlannedPath> {
    let dx = x1 - x0;
    let dy = y1 - y0;
    let d_dist = dx.hypot(dy);
    if d_dist < 0.5 { return None; }
    let d     = d_dist / r;
    let theta = mod2pi(dy.atan2(dx));
    let a = mod2pi(th0 - theta);
    let b = mod2pi(th1 - theta);

    let solvers: [fn(f64, f64, f64) -> Option<DubinsSol>; 6] = [
        dubins_lsl, dubins_rsr, dubins_lsr, dubins_rsl, dubins_rlr, dubins_lrl,
    ];

    let mut best_len = f64::INFINITY;
    let mut best: Option<DubinsSol> = None;
    for solver in &solvers {
        if let Some(sol) = solver(d, a, b) {
            let len = sol.length();
            if len.is_finite() && len >= 0.0 && len < best_len {
                best_len = len;
                best = Some(sol);
            }
        }
    }

    let sol = best?;
    Some(PlannedPath {
        length: (sol.t + sol.p + sol.q) * r,
        end_visual_heading,
        kind: PathKind::Dubins,
        x0, y0, th0, r,
        seg1: sol.t, seg2: sol.p, seg3: sol.q,
        types: sol.types,
        x1, y1, th1,
    })
}
