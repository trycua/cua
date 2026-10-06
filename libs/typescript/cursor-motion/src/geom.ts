/** A point in screen space (points, y down). */
export interface Pt {
  x: number;
  y: number;
}

/** Target rect `[x, y, width, height]`. */
export type Rect = [number, number, number, number];

export const pt = (x: number, y: number): Pt => ({ x, y });

export const dist = (a: Pt, b: Pt): number => Math.hypot(b.x - a.x, b.y - a.y);
export const lerp = (a: number, b: number, t: number): number => a + (b - a) * t;
export const lerpPt = (a: Pt, b: Pt, t: number): Pt => pt(lerp(a.x, b.x, t), lerp(a.y, b.y, t));
export const clamp = (v: number, lo: number, hi: number): number => Math.min(Math.max(v, lo), hi);

export function unit(a: Pt, b: Pt): Pt {
  let d = dist(a, b);
  if (d === 0) d = 1;
  return pt((b.x - a.x) / d, (b.y - a.y) / d);
}

/** 90 degrees counter-clockwise in screen space (y down). */
export const perp = (u: Pt): Pt => pt(-u.y, u.x);

const TAU = 2 * Math.PI;

/** Wrap an angle into `(-PI, PI]`. */
export function wrapAngle(a: number): number {
  let r = a % TAU;
  if (r > Math.PI) r -= TAU;
  if (r < -Math.PI) r += TAU;
  return r;
}

/**
 * Distance from the cursor's hotspot (the tip) to its anchor (the body),
 * along the heading, in points.
 */
export const POINTER_ANCHOR_OFFSET = 16;

/** Anchor (body point) that places the hotspot on `(x, y)` at `heading`. */
export function anchorForPointer(x: number, y: number, heading: number): [number, number] {
  return [
    x + Math.cos(heading) * POINTER_ANCHOR_OFFSET,
    y + Math.sin(heading) * POINTER_ANCHOR_OFFSET,
  ];
}

/** Hotspot for an anchor at `heading`. */
export function pointerForAnchor(x: number, y: number, heading: number): [number, number] {
  return [
    x - Math.cos(heading) * POINTER_ANCHOR_OFFSET,
    y - Math.sin(heading) * POINTER_ANCHOR_OFFSET,
  ];
}
