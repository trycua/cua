import { dist, lerp, lerpPt, perp, pt, unit, type Pt } from './geom';

/** A parametric path with an arc-length table, so speed curves act on distance. */
export class Path {
  readonly length: number;
  private readonly us: Float64Array;
  private readonly ss: Float64Array;
  private readonly p0: Pt;
  private readonly p1: Pt;
  private readonly t0: Pt;
  private readonly t1: Pt;

  constructor(
    private readonly f: (u: number) => Pt,
    n: number
  ) {
    this.us = new Float64Array(n + 1);
    this.ss = new Float64Array(n + 1);
    let prev = f(0);
    let total = 0;
    for (let i = 1; i <= n; i++) {
      const u = i / n;
      const p = f(u);
      total += Math.hypot(p.x - prev.x, p.y - prev.y);
      this.us[i] = u;
      this.ss[i] = total;
      prev = p;
    }
    this.length = total;
    this.p0 = f(0);
    this.p1 = f(1);
    this.t0 = unit(f(1e-3), this.p0);
    this.t1 = unit(f(1 - 1e-3), this.p1);
  }

  /** The point `frac` of the way along by distance; extrapolates past the ends. */
  atFraction(frac: number): Pt {
    const len = this.length;
    if (len < 1e-9) return this.f(Math.min(Math.max(frac, 0), 1));
    if (frac > 1) {
      return pt(this.p1.x + this.t1.x * (frac - 1) * len, this.p1.y + this.t1.y * (frac - 1) * len);
    }
    if (frac < 0) {
      return pt(this.p0.x + this.t0.x * -frac * len, this.p0.y + this.t0.y * -frac * len);
    }
    const target = frac * len;
    let lo = 0;
    let hi = this.ss.length - 1;
    while (hi - lo > 1) {
      const mid = (lo + hi) >> 1;
      if (this.ss[mid]! < target) lo = mid;
      else hi = mid;
    }
    let span = this.ss[hi]! - this.ss[lo]!;
    if (span === 0) span = 1;
    return this.f(lerp(this.us[lo]!, this.us[hi]!, (target - this.ss[lo]!) / span));
  }
}

const cubic = (p0: Pt, p1: Pt, p2: Pt, p3: Pt) => (u: number) => {
  const v = 1 - u;
  const a = v * v * v;
  const b = 3 * v * v * u;
  const c = 3 * v * u * u;
  const d = u * u * u;
  return pt(a * p0.x + b * p1.x + c * p2.x + d * p3.x, a * p0.y + b * p1.y + c * p2.y + d * p3.y);
};

const quad = (p0: Pt, p1: Pt, p2: Pt) => (u: number) => {
  const v = 1 - u;
  return pt(
    v * v * p0.x + 2 * v * u * p1.x + u * u * p2.x,
    v * v * p0.y + 2 * v * u * p1.y + u * u * p2.y
  );
};

/** Arc knobs of the Cua bezier. */
export interface ArcShape {
  /** First control point's distance from the start, fraction of the chord. [0, 1] */
  startHandle: number;
  /** Second control point's distance from the end. [0, 1] */
  endHandle: number;
  /** Sideways bend, fraction of the chord; the sign picks the side. */
  arcSize: number;
  /** Where the bend peaks: -1 near the start, +1 near the end. */
  arcFlow: number;
}

/** The Cua bezier as a path. */
export function cuaPath(a: Pt, b: Pt, shape: ArcShape): Path {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const len = Math.max(Math.hypot(dx, dy), 1);
  const px = -dy / len;
  const py = dx / len;
  const deflection = len * shape.arcSize;
  const flow = (shape.arcFlow + 1) / 2;
  const c1d = deflection * (1 - 0.5 * flow);
  const c2d = deflection * (1 - 0.5 * (1 - flow));
  const c1 = pt(a.x + dx * shape.startHandle + px * c1d, a.y + dy * shape.startHandle + py * c1d);
  const c2 = pt(b.x - dx * shape.endHandle + px * c2d, b.y - dy * shape.endHandle + py * c2d);
  return new Path(cubic(a, c1, c2, b), 256);
}

/** Gentle single-sided quadratic curve. */
export function bowPath(a: Pt, b: Pt, amount: number): Path {
  const d = dist(a, b);
  const n = perp(unit(a, b));
  const m = lerpPt(a, b, 0.5);
  return new Path(quad(a, pt(m.x + n.x * amount * d, m.y + n.y * amount * d), b), 256);
}

export const straightPath = (a: Pt, b: Pt): Path => new Path((u) => lerpPt(a, b, u), 8);

/** Chord side that bends paths upward for horizontal moves. */
export const naturalSide = (a: Pt, b: Pt): number => (unit(a, b).x >= 0 ? -1 : 1);
