/** Dubins arc-straight-arc planner of the `classic` glide (port of `dubins.rs`). */

type Seg = 'L' | 'R' | 'S';

export interface PathState {
  x: number;
  y: number;
  heading: number;
}

const TAU = 2 * Math.PI;

function mod2pi(x: number): number {
  const r = x - TAU * Math.floor(x / TAU);
  return r < 0 ? r + TAU : r;
}

interface Sol {
  t: number;
  p: number;
  q: number;
  types: [Seg, Seg, Seg];
}

function lsl(d: number, a: number, b: number): Sol | null {
  const tmp0 = d + Math.sin(a) - Math.sin(b);
  const p2 = 2 + d * d - 2 * Math.cos(a - b) + 2 * d * (Math.sin(a) - Math.sin(b));
  if (p2 < 0) return null;
  const tmp1 = Math.atan2(Math.cos(b) - Math.cos(a), tmp0);
  return { t: mod2pi(-a + tmp1), p: Math.sqrt(p2), q: mod2pi(b - tmp1), types: ['L', 'S', 'L'] };
}

function rsr(d: number, a: number, b: number): Sol | null {
  const tmp0 = d - Math.sin(a) + Math.sin(b);
  const p2 = 2 + d * d - 2 * Math.cos(a - b) + 2 * d * (Math.sin(b) - Math.sin(a));
  if (p2 < 0) return null;
  const tmp1 = Math.atan2(Math.cos(a) - Math.cos(b), tmp0);
  return { t: mod2pi(a - tmp1), p: Math.sqrt(p2), q: mod2pi(-b + tmp1), types: ['R', 'S', 'R'] };
}

function lsr(d: number, a: number, b: number): Sol | null {
  const p2 = -2 + d * d + 2 * Math.cos(a - b) + 2 * d * (Math.sin(a) + Math.sin(b));
  if (p2 < 0) return null;
  const p = Math.sqrt(p2);
  const tmp1 =
    Math.atan2(-(Math.cos(a) + Math.cos(b)), d + Math.sin(a) + Math.sin(b)) - Math.atan2(-2, p);
  return { t: mod2pi(-a + tmp1), p, q: mod2pi(-mod2pi(b) + tmp1), types: ['L', 'S', 'R'] };
}

function rsl(d: number, a: number, b: number): Sol | null {
  const p2 = d * d - 2 + 2 * Math.cos(a - b) - 2 * d * (Math.sin(a) + Math.sin(b));
  if (p2 < 0) return null;
  const p = Math.sqrt(p2);
  const tmp1 =
    Math.atan2(Math.cos(a) + Math.cos(b), d - Math.sin(a) - Math.sin(b)) - Math.atan2(2, p);
  return { t: mod2pi(a - tmp1), p, q: mod2pi(b - tmp1), types: ['R', 'S', 'L'] };
}

function rlr(d: number, a: number, b: number): Sol | null {
  const tmp = (6 - d * d + 2 * Math.cos(a - b) + 2 * d * (Math.sin(a) - Math.sin(b))) / 8;
  if (Math.abs(tmp) > 1) return null;
  const p = mod2pi(TAU - Math.acos(tmp));
  const t = mod2pi(
    a - Math.atan2(Math.cos(a) - Math.cos(b), d - Math.sin(a) + Math.sin(b)) + p / 2
  );
  return { t, p, q: mod2pi(a - b - t + p), types: ['R', 'L', 'R'] };
}

function lrl(d: number, a: number, b: number): Sol | null {
  const tmp = (6 - d * d + 2 * Math.cos(a - b) + 2 * d * (Math.sin(b) - Math.sin(a))) / 8;
  if (Math.abs(tmp) > 1) return null;
  const p = mod2pi(TAU - Math.acos(tmp));
  const t = mod2pi(
    -a + Math.atan2(-Math.cos(a) + Math.cos(b), d + Math.sin(a) - Math.sin(b)) + p / 2
  );
  return { t, p, q: mod2pi(mod2pi(b) - a - t + p), types: ['L', 'R', 'L'] };
}

export interface PlannedPath {
  length: number;
  sample(distance: number): PathState;
}

/** Plan a Dubins path from `(x0, y0)` heading `th0` to `(x1, y1)` heading `th1`. */
export function planDubins(
  x0: number,
  y0: number,
  th0: number,
  x1: number,
  y1: number,
  th1: number,
  turnRadius: number
): PlannedPath {
  const r = Math.max(turnRadius, 1);
  const dx = x1 - x0;
  const dy = y1 - y0;
  const dd = Math.hypot(dx, dy);
  let best: Sol | null = null;
  if (dd >= 0.5) {
    const d = dd / r;
    const theta = mod2pi(Math.atan2(dy, dx));
    const a = mod2pi(th0 - theta);
    const b = mod2pi(th1 - theta);
    let bestLen = Infinity;
    for (const solver of [lsl, rsr, lsr, rsl, rlr, lrl]) {
      const sol = solver(d, a, b);
      if (!sol) continue;
      const len = sol.t + sol.p + sol.q;
      if (Number.isFinite(len) && len >= 0 && len < bestLen) {
        bestLen = len;
        best = sol;
      }
    }
  }
  if (!best) {
    const length = Math.max(dd, 1);
    return {
      length,
      sample(s) {
        const u = Math.min(Math.max(s / Math.max(length, 1), 0), 1);
        let diff = th1 - th0;
        while (diff > Math.PI) diff -= TAU;
        while (diff < -Math.PI) diff += TAU;
        return { x: x0 + (x1 - x0) * u, y: y0 + (y1 - y0) * u, heading: th0 + diff * u };
      },
    };
  }
  const sol = best;
  const l1 = sol.t * r;
  const l2 = sol.p * r;
  const l3 = sol.q * r;
  return {
    length: (sol.t + sol.p + sol.q) * r,
    sample(sIn) {
      if (sIn <= 0) return { x: x0, y: y0, heading: th0 };
      const s = Math.min(sIn, l1 + l2 + l3);
      let x = x0;
      let y = y0;
      let th = th0;
      const advance = (len: number, seg: Seg) => {
        if (seg === 'S') {
          x += Math.cos(th) * len;
          y += Math.sin(th) * len;
        } else {
          const dth = (len / r) * (seg === 'L' ? 1 : -1);
          const side = seg === 'L' ? Math.PI / 2 : -Math.PI / 2;
          const cx = x + Math.cos(th + side) * r;
          const cy = y + Math.sin(th + side) * r;
          const ang = Math.atan2(y - cy, x - cx);
          x = cx + Math.cos(ang + dth) * r;
          y = cy + Math.sin(ang + dth) * r;
          th += dth;
        }
      };
      if (s <= l1) {
        advance(s, sol.types[0]);
        return { x, y, heading: th };
      }
      advance(l1, sol.types[0]);
      if (s <= l1 + l2) {
        advance(s - l1, sol.types[1]);
        return { x, y, heading: th };
      }
      advance(l2, sol.types[1]);
      advance(s - l1 - l2, sol.types[2]);
      return { x, y, heading: th };
    },
  };
}
