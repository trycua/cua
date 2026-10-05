// Geometry and easing primitives shared by every candidate.

// Sample period for every generated trajectory (120 Hz).
export const DT_MS = 1000 / 120;

export const TAU = Math.PI * 2;
export const clamp = (v, lo, hi) => Math.min(hi, Math.max(lo, v));
export const lerp = (a, b, t) => a + (b - a) * t;
export const dist = (a, b) => Math.hypot(b.x - a.x, b.y - a.y);
export const add = (a, b) => ({ x: a.x + b.x, y: a.y + b.y });
export const sub = (a, b) => ({ x: a.x - b.x, y: a.y - b.y });
export const scale = (a, k) => ({ x: a.x * k, y: a.y * k });
export const lerpPt = (a, b, t) => ({ x: lerp(a.x, b.x, t), y: lerp(a.y, b.y, t) });
export const unit = (a, b) => {
  const d = dist(a, b) || 1;
  return { x: (b.x - a.x) / d, y: (b.y - a.y) / d };
};
// 90 degrees counter-clockwise in screen space (y down).
export const perp = (u) => ({ x: -u.y, y: u.x });
export const wrapAngle = (a) => {
  let r = a % TAU;
  if (r > Math.PI) r -= TAU;
  if (r < -Math.PI) r += TAU;
  return r;
};

// ---------------------------------------------------------------------------
// Time-normalised position profiles s(tau), tau and s in [0, 1].
// ---------------------------------------------------------------------------

export const ease = {
  linear: (t) => t,
  inOutSine: (t) => 0.5 - 0.5 * Math.cos(Math.PI * t),
  inOutQuad: (t) => (t < 0.5 ? 2 * t * t : 1 - (-2 * t + 2) ** 2 / 2),
  inOutCubic: (t) => (t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2),
  inOutQuint: (t) => (t < 0.5 ? 16 * t ** 5 : 1 - (-2 * t + 2) ** 5 / 2),
  outCubic: (t) => 1 - (1 - t) ** 3,
  outQuart: (t) => 1 - (1 - t) ** 4,
  outQuint: (t) => 1 - (1 - t) ** 5,
  outExpo: (t) => (t >= 1 ? 1 : (1 - 2 ** (-10 * t)) / (1 - 2 ** -10)),
  inOutExpo: (t) => {
    if (t <= 0) return 0;
    if (t >= 1) return 1;
    return t < 0.5 ? 2 ** (20 * t - 10) / 2 : (2 - 2 ** (-20 * t + 10)) / 2;
  },
  // Hogan & Flash minimum-jerk: the classic human point-to-point profile.
  minJerk: (t) => t * t * t * (10 - 15 * t + 6 * t * t),
  // Smootherstep, same as minJerk; kept as an alias for the Cua renderer's naming.
  smootherstep: (t) => t * t * t * (t * (6 * t - 15) + 10),
  // Overshooting ease-out ("back").
  outBack: (t, k = 1.70158) => 1 + (k + 1) * (t - 1) ** 3 + k * (t - 1) ** 2,
  // Anticipation ease-in ("back" at the start).
  inOutBack: (t, k = 1.70158 * 1.525) =>
    t < 0.5
      ? ((2 * t) ** 2 * ((k + 1) * 2 * t - k)) / 2
      : ((2 * t - 2) ** 2 * ((k + 1) * (t * 2 - 2) + k) + 2) / 2,
};

// Minimum-jerk with an asymmetric velocity peak: warp time so the bell peaks
// at `peak` (0.5 is symmetric). Humans typically peak at 0.38-0.45.
export function asymmetricMinJerk(peak = 0.42) {
  // Piecewise-linear time warp keeps s(0)=0, s(1)=1 and C1 continuity is
  // restored by blending with a smooth power warp.
  const g = Math.log(0.5) / Math.log(clamp(peak, 0.2, 0.8));
  return (t) => ease.minJerk(t ** g);
}

// Cumulative lognormal: the integral of one sigma-lognormal stroke.
export function lognormalCdf(t, t0, mu, sigma) {
  if (t <= t0) return 0;
  return 0.5 * (1 + erf((Math.log(t - t0) - mu) / (sigma * Math.SQRT2)));
}

export function lognormalPdf(t, t0, mu, sigma) {
  if (t <= t0) return 0;
  const x = t - t0;
  return Math.exp(-((Math.log(x) - mu) ** 2) / (2 * sigma * sigma)) / (sigma * Math.sqrt(TAU) * x);
}

// Abramowitz-Stegun 7.1.26, max error 1.5e-7.
export function erf(x) {
  const s = Math.sign(x);
  const a = Math.abs(x);
  const t = 1 / (1 + 0.3275911 * a);
  const y =
    1 -
    ((((1.061405429 * t - 1.453152027) * t + 1.421413741) * t - 0.284496736) * t + 0.254829592) *
      t *
      Math.exp(-a * a);
  return s * y;
}

// ---------------------------------------------------------------------------
// Paths p(u), u in [0, 1]. Every path object exposes `at(u)` and `length`, and
// an arc-length reparameterisation so profiles act on distance, not on u.
// ---------------------------------------------------------------------------

export function arcLengthTable(fn, n = 256) {
  const us = [0];
  const ss = [0];
  let prev = fn(0);
  let total = 0;
  for (let i = 1; i <= n; i++) {
    const u = i / n;
    const p = fn(u);
    total += Math.hypot(p.x - prev.x, p.y - prev.y);
    us.push(u);
    ss.push(total);
    prev = p;
  }
  return { us, ss, total };
}

export function makePath(fn, n = 256) {
  const table = arcLengthTable(fn, n);
  const length = table.total;
  const p0 = fn(0);
  const p1 = fn(1);
  const t0 = unit(fn(1e-3), p0); // points backwards out of the start
  const t1 = unit(fn(1 - 1e-3), p1); // points forwards out of the end
  return {
    length,
    at: fn,
    // Position at fraction `f` of arc length. Fractions outside [0, 1]
    // extrapolate along the end tangents, which is how overshoot and
    // anticipation profiles leave the path and come back.
    atFraction(f) {
      if (length < 1e-9) return fn(clamp(f, 0, 1));
      if (f > 1) return { x: p1.x + t1.x * (f - 1) * length, y: p1.y + t1.y * (f - 1) * length };
      if (f < 0) return { x: p0.x + t0.x * -f * length, y: p0.y + t0.y * -f * length };
      const target = f * length;
      const { us, ss } = table;
      let lo = 0;
      let hi = ss.length - 1;
      while (hi - lo > 1) {
        const mid = (lo + hi) >> 1;
        if (ss[mid] < target) lo = mid;
        else hi = mid;
      }
      const span = ss[hi] - ss[lo] || 1;
      const u = lerp(us[lo], us[hi], (target - ss[lo]) / span);
      return fn(u);
    },
  };
}

export function linePath(a, b) {
  return makePath((u) => lerpPt(a, b, u), 8);
}

export function cubic(p0, p1, p2, p3) {
  return (u) => {
    const v = 1 - u;
    const a = v * v * v;
    const b = 3 * v * v * u;
    const c = 3 * v * u * u;
    const d = u * u * u;
    return {
      x: a * p0.x + b * p1.x + c * p2.x + d * p3.x,
      y: a * p0.y + b * p1.y + c * p2.y + d * p3.y,
    };
  };
}

export function quad(p0, p1, p2) {
  return (u) => {
    const v = 1 - u;
    return {
      x: v * v * p0.x + 2 * v * u * p1.x + u * u * p2.x,
      y: v * v * p0.y + 2 * v * u * p1.y + u * u * p2.y,
    };
  };
}

// The Cua Driver bezier formula (bezier.rs `build_motion_bezier`): handles
// along the chord plus a perpendicular deflection split by `arcFlow`.
export function cuaBezier(
  a,
  b,
  { startHandle = 0.3, endHandle = 0.3, arcSize = 0.25, arcFlow = 0 } = {}
) {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len = Math.max(1, Math.hypot(dx, dy));
  const px = -dy / len;
  const py = dx / len;
  const deflection = len * arcSize;
  const flow = (arcFlow + 1) / 2;
  const c1d = deflection * (1 - 0.5 * flow);
  const c2d = deflection * (1 - 0.5 * (1 - flow));
  const c1 = { x: a.x + dx * startHandle + px * c1d, y: a.y + dy * startHandle + py * c1d };
  const c2 = { x: b.x - dx * endHandle + px * c2d, y: b.y - dy * endHandle + py * c2d };
  return cubic(a, c1, c2, b);
}

// Circular arc from a to b bulging by `bulge` (sagitta / chord). Sign picks side.
export function arcPath(a, b, bulge) {
  const chord = dist(a, b);
  if (chord < 1e-6 || Math.abs(bulge) < 1e-4) return (u) => lerpPt(a, b, u);
  const h = bulge * chord; // sagitta
  const r = (h * h + (chord / 2) ** 2) / (2 * Math.abs(h));
  const m = lerpPt(a, b, 0.5);
  const n = perp(unit(a, b));
  const sgn = Math.sign(h);
  // Center sits on the opposite side of the bulge.
  const c = { x: m.x - n.x * sgn * (r - Math.abs(h)), y: m.y - n.y * sgn * (r - Math.abs(h)) };
  const a0 = Math.atan2(a.y - c.y, a.x - c.x);
  let a1 = Math.atan2(b.y - c.y, b.x - c.x);
  // Choose the sweep that passes through the bulge point.
  const apex = { x: m.x + n.x * h, y: m.y + n.y * h };
  const aApex = Math.atan2(apex.y - c.y, apex.x - c.x);
  let sweep = wrapAngle(a1 - a0);
  const apexRel = wrapAngle(aApex - a0);
  const within = sweep > 0 ? apexRel > 0 && apexRel < sweep : apexRel < 0 && apexRel > sweep;
  if (!within) sweep = sweep > 0 ? sweep - TAU : sweep + TAU;
  return (u) => {
    const ang = a0 + sweep * u;
    return { x: c.x + r * Math.cos(ang), y: c.y + r * Math.sin(ang) };
  };
}

// Centripetal Catmull-Rom through points (alpha 0.5), as one path function.
export function catmullRom(points, alpha = 0.5) {
  if (points.length < 2) return () => ({ ...points[0] });
  const pts = [
    add(points[0], sub(points[0], points[1])),
    ...points,
    add(points[points.length - 1], sub(points[points.length - 1], points[points.length - 2])),
  ];
  const segs = points.length - 1;
  const tj = (ti, p0, p1) => ti + Math.max(1e-6, dist(p0, p1) ** alpha);
  return (u) => {
    const f = clamp(u, 0, 1) * segs;
    const i = Math.min(segs - 1, Math.floor(f));
    const local = f - i;
    const p0 = pts[i];
    const p1 = pts[i + 1];
    const p2 = pts[i + 2];
    const p3 = pts[i + 3];
    const t0 = 0;
    const t1 = tj(t0, p0, p1);
    const t2 = tj(t1, p1, p2);
    const t3 = tj(t2, p2, p3);
    const t = lerp(t1, t2, local);
    const A1 = lerpPt(p0, p1, (t - t0) / (t1 - t0));
    const A2 = lerpPt(p1, p2, (t - t1) / (t2 - t1));
    const A3 = lerpPt(p2, p3, (t - t2) / (t3 - t2));
    const B1 = lerpPt(A1, A2, (t - t0) / (t2 - t0));
    const B2 = lerpPt(A2, A3, (t - t1) / (t3 - t1));
    return lerpPt(B1, B2, (t - t1) / (t2 - t1));
  };
}

// ---------------------------------------------------------------------------
// Dubins (arc-straight-arc) planner: a JS port of cursor-overlay's
// path_planner.rs, used to reproduce today's Cua Driver glide.
// ---------------------------------------------------------------------------

const mod2pi = (x) => {
  const r = x - TAU * Math.floor(x / TAU);
  return r < 0 ? r + TAU : r;
};

const dubinsSolvers = [
  (d, a, b) => {
    const p2 = 2 + d * d - 2 * Math.cos(a - b) + 2 * d * (Math.sin(a) - Math.sin(b));
    if (p2 < 0) return null;
    const t1 = Math.atan2(Math.cos(b) - Math.cos(a), d + Math.sin(a) - Math.sin(b));
    return { t: mod2pi(-a + t1), p: Math.sqrt(p2), q: mod2pi(b - t1), types: 'LSL' };
  },
  (d, a, b) => {
    const p2 = 2 + d * d - 2 * Math.cos(a - b) + 2 * d * (Math.sin(b) - Math.sin(a));
    if (p2 < 0) return null;
    const t1 = Math.atan2(Math.cos(a) - Math.cos(b), d - Math.sin(a) + Math.sin(b));
    return { t: mod2pi(a - t1), p: Math.sqrt(p2), q: mod2pi(-b + t1), types: 'RSR' };
  },
  (d, a, b) => {
    const p2 = -2 + d * d + 2 * Math.cos(a - b) + 2 * d * (Math.sin(a) + Math.sin(b));
    if (p2 < 0) return null;
    const p = Math.sqrt(p2);
    const t1 =
      Math.atan2(-(Math.cos(a) + Math.cos(b)), d + Math.sin(a) + Math.sin(b)) - Math.atan2(-2, p);
    return { t: mod2pi(-a + t1), p, q: mod2pi(-mod2pi(b) + t1), types: 'LSR' };
  },
  (d, a, b) => {
    const p2 = d * d - 2 + 2 * Math.cos(a - b) - 2 * d * (Math.sin(a) + Math.sin(b));
    if (p2 < 0) return null;
    const p = Math.sqrt(p2);
    const t1 =
      Math.atan2(Math.cos(a) + Math.cos(b), d - Math.sin(a) - Math.sin(b)) - Math.atan2(2, p);
    return { t: mod2pi(a - t1), p, q: mod2pi(b - t1), types: 'RSL' };
  },
];

export function dubinsPath(a, th0, b, th1, radius) {
  const r = Math.max(1, radius);
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const D = Math.hypot(dx, dy);
  if (D < 0.5) return null;
  const d = D / r;
  const theta = mod2pi(Math.atan2(dy, dx));
  const al = mod2pi(th0 - theta);
  const be = mod2pi(th1 - theta);
  let best = null;
  for (const solve of dubinsSolvers) {
    const s = solve(d, al, be);
    if (
      s &&
      Number.isFinite(s.t + s.p + s.q) &&
      (!best || s.t + s.p + s.q < best.t + best.p + best.q)
    )
      best = s;
  }
  if (!best) return null;
  const segs = [best.t * r, best.p * r, best.q * r];
  const total = segs[0] + segs[1] + segs[2];
  const advance = (state, len, type) => {
    let { x, y, th } = state;
    if (type === 'S') {
      x += Math.cos(th) * len;
      y += Math.sin(th) * len;
    } else {
      const dir = type === 'L' ? 1 : -1;
      const dth = (len / r) * dir;
      const cx = x + Math.cos(th + (dir * Math.PI) / 2) * r;
      const cy = y + Math.sin(th + (dir * Math.PI) / 2) * r;
      const ang = Math.atan2(y - cy, x - cx);
      x = cx + Math.cos(ang + dth) * r;
      y = cy + Math.sin(ang + dth) * r;
      th += dth;
    }
    return { x, y, th };
  };
  const fn = (u) => {
    let s = clamp(u, 0, 1) * total;
    let st = { x: a.x, y: a.y, th: th0 };
    for (let i = 0; i < 3; i++) {
      const len = Math.min(s, segs[i]);
      st = advance(st, len, best.types[i]);
      s -= len;
      if (s <= 0) break;
    }
    return { x: st.x, y: st.y };
  };
  return { fn, length: total };
}
