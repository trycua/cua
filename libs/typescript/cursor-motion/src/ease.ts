/** Speed curves: how far along the path the cursor is at each moment. */

export type EaseName =
  | 'linear'
  | 'min_jerk'
  | 'smootherstep'
  | 'in_out_cubic'
  | 'in_out_sine'
  | 'out_cubic';

export type Ease =
  | EaseName
  | { type: EaseName }
  | { type: 'cubic_bezier'; x1: number; y1: number; x2: number; y2: number };

export const EASE_NAMES: EaseName[] = [
  'linear',
  'min_jerk',
  'smootherstep',
  'in_out_cubic',
  'in_out_sine',
  'out_cubic',
];

export const linear = (t: number): number => t;
/** Minimum-jerk: the profile of a relaxed human reach. */
export const minJerk = (t: number): number => t * t * t * (10 - 15 * t + 6 * t * t);
export const smootherstep = (t: number): number => t * t * t * (t * (6 * t - 15) + 10);
export const inOutCubic = (t: number): number =>
  t < 0.5 ? 4 * t * t * t : 1 - (-2 * t + 2) ** 3 / 2;
export const inOutSine = (t: number): number => 0.5 - 0.5 * Math.cos(Math.PI * t);
export const outCubic = (t: number): number => 1 - (1 - t) ** 3;

/** CSS-style `cubic-bezier(x1, y1, x2, y2)`. */
export function cubicBezier(x1: number, y1: number, x2: number, y2: number, t: number): number {
  if (t <= 0) return 0;
  if (t >= 1) return 1;
  const cx1 = Math.min(Math.max(x1, 0), 1);
  const cx2 = Math.min(Math.max(x2, 0), 1);
  const coord = (a: number, b: number, u: number) => {
    const v = 1 - u;
    return 3 * v * v * u * a + 3 * v * u * u * b + u * u * u;
  };
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < 52; i++) {
    const mid = 0.5 * (lo + hi);
    if (coord(cx1, cx2, mid) < t) lo = mid;
    else hi = mid;
  }
  return coord(y1, y2, 0.5 * (lo + hi));
}

const NAMED: Record<EaseName, (t: number) => number> = {
  linear,
  min_jerk: minJerk,
  smootherstep,
  in_out_cubic: inOutCubic,
  in_out_sine: inOutSine,
  out_cubic: outCubic,
};

/** The function behind an `Ease` value. */
export function easeFn(ease: Ease): (t: number) => number {
  if (typeof ease === 'string') return NAMED[ease];
  if (ease.type === 'cubic_bezier') {
    const { x1, y1, x2, y2 } = ease as Extract<Ease, { type: 'cubic_bezier' }>;
    return (t) => cubicBezier(x1, y1, x2, y2, t);
  }
  return NAMED[ease.type];
}

/** Base plus a smooth bump that pushes `over` past the end, peaking at `overAt`. */
export function bumpProfile(base: (t: number) => number, over: number, overAt: number) {
  const a = Math.max(overAt * 10, 1.5);
  const b = Math.max((1 - overAt) * 10, 1.5);
  const peak = (a / (a + b)) ** a * (b / (a + b)) ** b;
  return (tau: number) => base(tau) + over * ((tau ** a * (1 - tau) ** b) / peak);
}

/** Damped wobble around the end from `start` on. */
export function wobbleProfile(
  base: (t: number) => number,
  amp: number,
  cycles: number,
  decay: number,
  start: number
) {
  return (tau: number) => {
    if (tau <= start) return base(tau);
    const u = (tau - start) / (1 - start);
    const ramp = smootherstep(Math.min(u / 0.18, 1));
    return (
      base(tau) +
      (amp * ramp * Math.exp(-decay * u) * Math.sin(2 * Math.PI * cycles * u) * (1 - u) ** 2) /
        Math.max(Math.exp(-decay * 0.12) * 0.77, 1e-6)
    );
  };
}
