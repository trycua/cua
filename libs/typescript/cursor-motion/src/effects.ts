/**
 * Geometry of the motion effects at a moment of a move: comet trail, speed
 * glow, magnet glow, click ripple and click squish. The renderer paints it.
 */
import { POINTER_ANCHOR_OFFSET, type Rect } from './geom';
import type { Trajectory } from './plan';
import type { ResolvedEffects } from './style';

export interface TrailSpec {
  /** How far back in time the trail reaches, seconds. */
  secs: number;
  /** Path length (pt) at which the trail reaches full strength. */
  fadeLen: number;
  /** Segments. */
  steps: number;
  /** Stroke width at the tail and the head, pt. */
  tailWidth: number;
  headWidth: number;
  /** Opacity at the head; falls off with the square of the position. */
  alpha: number;
  /** How far behind the tip, along the arrow's axis, the trail starts, pt. */
  anchorOffset: number;
}

export const DEFAULT_TRAIL: Readonly<TrailSpec> = Object.freeze({
  secs: 0.18,
  fadeLen: 60,
  steps: 26,
  tailWidth: 2,
  headWidth: 12,
  alpha: 0.38,
  anchorOffset: POINTER_ANCHOR_OFFSET,
});

export const MAGNET_SECS = 0.7;
export const MAGNET_INFLATE = 6;
export const RIPPLE_SECS = 0.52;
export const CLICK_FX_SECS = 0.55;
export const SQUISH = 0.12;

export interface Glow {
  x: number;
  y: number;
  r: number;
  alpha: number;
}
export interface TrailSeg {
  a: [number, number];
  b: [number, number];
  width: number;
  alpha: number;
}
export interface Magnet {
  rect: Rect;
  glow: number;
}
export interface Ripple {
  x: number;
  y: number;
  r: number;
  width: number;
  alpha: number;
}
export interface EffectFrame {
  glow: Glow | null;
  trail: TrailSeg[];
  magnet: Magnet | null;
  ripple: Ripple | null;
  /** Scale-down of the cursor, 0 = none. */
  squish: number;
}

export const emptyFrame = (): EffectFrame => ({
  glow: null,
  trail: [],
  magnet: null,
  ripple: null,
  squish: 0,
});

/** Speed glow at `t` around `pos` (the cursor's anchor). */
export function glow(traj: Trajectory, t: number, pos: [number, number]): Glow | null {
  if (t >= traj.duration()) return null;
  const [vx, vy] = traj.velocityAt(t);
  const speed = Math.hypot(vx, vy);
  const alpha = Math.min(speed * 0.00014, 0.42);
  if (alpha <= 0.02) return null;
  const off = Math.min(speed * 0.009, 18);
  return {
    x: pos[0] - (vx / speed) * off,
    y: pos[1] - (vy / speed) * off,
    r: 30 * (1 + Math.min(speed * 0.00024, 0.44)),
    alpha,
  };
}

/** Comet trail at `t`, tail first, following the arrow's body. */
export function trail(traj: Trajectory, t: number): TrailSeg[] {
  const spec = traj.trail;
  const steps = Math.max(spec.steps, 1);
  const pts: [number, number][] = [];
  for (let i = 0; i <= steps; i++) {
    const s = traj.sampleAt(t - spec.secs + spec.secs * (i / steps));
    pts.push([
      s.x + Math.cos(s.heading) * spec.anchorOffset,
      s.y + Math.sin(s.heading) * spec.anchorOffset,
    ]);
  }
  let length = 0;
  for (let i = 1; i <= steps; i++) {
    length += Math.hypot(pts[i]![0] - pts[i - 1]![0], pts[i]![1] - pts[i - 1]![1]);
  }
  const fade = Math.min(length / spec.fadeLen, 1);
  const out: TrailSeg[] = [];
  for (let i = 1; i <= steps; i++) {
    const a = pts[i - 1]!;
    const b = pts[i]!;
    const k = i / steps;
    if (Math.hypot(b[0] - a[0], b[1] - a[1]) > 0.3) {
      out.push({
        a,
        b,
        width: spec.tailWidth + (spec.headWidth - spec.tailWidth) * k,
        alpha: spec.alpha * k * k * fade,
      });
    }
  }
  return out;
}

/** Magnet glow at `t`, after a lock-on. */
export function magnet(traj: Trajectory, t: number): Magnet | null {
  if (traj.snapT === null) return null;
  const age = t - traj.snapT;
  if (!(age >= 0 && age < MAGNET_SECS)) return null;
  const end = traj.end();
  return {
    rect: traj.targetKnown ? traj.target : [end.x - 12, end.y - 12, 24, 24],
    glow: 1 - age / MAGNET_SECS,
  };
}

/** Click ripple `age` seconds after a click at `point`. */
export function ripple(age: number, point: [number, number]): Ripple | null {
  if (age >= RIPPLE_SECS) return null;
  const k = age / RIPPLE_SECS;
  const easeOut = 1 - (1 - k) ** 3;
  return {
    x: point[0],
    y: point[1],
    r: 8 + 44 * easeOut,
    width: 4 * (1 - k) + 1,
    alpha: 0.75 * (1 - k),
  };
}

/** Click squish `age` seconds after a click. */
export function squish(age: number): number {
  const PRESS = 0.09;
  if (age < PRESS) return SQUISH * Math.min(age / 0.05, 1);
  const after = age - PRESS;
  return (
    SQUISH *
    Math.max(Math.cos(Math.min(after / 0.22, 1) * Math.PI * 1.5), 0) *
    Math.max(1 - after / 0.22, 0)
  );
}

/** The move's effects at `t`. `pos` is the anchor; `blend` = can draw translucency. */
export function motionFrame(
  traj: Trajectory,
  t: number,
  pos: [number, number],
  blend = true
): EffectFrame {
  const fx = traj.effects;
  return {
    glow: fx.glow && blend ? glow(traj, t, pos) : null,
    trail: fx.trail && blend ? trail(traj, t) : [],
    magnet: fx.magnet ? magnet(traj, t) : null,
    ripple: null,
    squish: 0,
  };
}

/** Add the click effects `age` seconds after a click at `point`. */
export function addClick(
  frame: EffectFrame,
  effects: ResolvedEffects,
  age: number,
  point: [number, number]
): void {
  if (effects.ripple) frame.ripple = ripple(age, point);
  if (effects.squish) frame.squish = squish(age);
}

/** When a finished trajectory can be dropped (trail caught up, magnet faded). */
export function linger(traj: Trajectory): number {
  let end = traj.duration();
  if (traj.effects.trail) end += traj.trail.secs;
  if (traj.effects.magnet && traj.snapT !== null) end = Math.max(end, traj.snapT + MAGNET_SECS);
  return end;
}

/** Effect colour: the cursor fill lifted toward white. */
export function effectRgb(fill: [number, number, number]): [number, number, number] {
  const lift = (c: number) => Math.round(c + (255 - c) * 0.45);
  return [lift(fill[0]), lift(fill[1]), lift(fill[2])];
}
