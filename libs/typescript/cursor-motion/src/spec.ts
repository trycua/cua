import type { Ease } from './ease';
import { DEFAULT_TRAIL, type TrailSpec } from './effects';
import { DEFAULT_PARAMS, motionParams, resolvedEffects, type MotionParams } from './params';
import type { ArcShape } from './path';
import type { MotionStyle, ResolvedEffects } from './style';

/** Path shape. A positive arc or bow bends to the natural side (up for horizontal moves). */
export type PathShape =
  | { type: 'straight' }
  | ({ type: 'arc' } & ArcShape)
  | { type: 'bow'; amount: number };

/** What happens at the end of the glide. */
export type Settle =
  | { type: 'none' }
  /** Push `amount` of the distance (at most `maxPt`) past the target, peaking at `at`, and return. */
  | { type: 'follow_through'; amount: number; maxPt: number; at: number }
  /** Reach the target by `glideEnd`, then a damped wobble from `start` on. */
  | {
      type: 'spring';
      amount: number;
      maxPt: number;
      cycles: number;
      decay: number;
      start: number;
      glideEnd: number;
    };

/** How long the move takes. */
export type Duration =
  | { type: 'fixed'; ms: number }
  /** `(a + b log2(D / W + 1)).clamp(minMs, maxMs) * scale`; W = target's smaller side (>= 4). */
  | { type: 'fitts'; a: number; b: number; minMs: number; maxMs: number; scale: number }
  /** `(baseMs + msPerPt * D).clamp(minMs, maxMs)`. */
  | { type: 'distance'; baseMs: number; msPerPt: number; minMs: number; maxMs: number };

/** `tangent`: the tip leads the motion. `fixed`: the arrow keeps its rest pose. */
export type Heading = 'tangent' | 'fixed';

/** A complete custom motion. */
export interface MotionSpec {
  path: PathShape;
  ease: Ease;
  settle: Settle;
  duration: Duration;
  heading: Heading;
  effects: ResolvedEffects;
  trail: TrailSpec;
}

/** The Fitts model the director's-cut styles share. */
export const fittsDuration = (scale: number): Duration => ({
  type: 'fitts',
  a: 150,
  b: 120,
  minMs: 300,
  maxMs: 1000,
  scale,
});

export function durationMs(d: Duration, distance: number, width: number): number {
  const clamp = (v: number, lo: number, hi: number) => Math.min(Math.max(v, lo), hi);
  switch (d.type) {
    case 'fixed':
      return d.ms;
    case 'fitts':
      return (
        clamp(d.a + d.b * Math.log2(distance / Math.max(width, 4) + 1), d.minMs, d.maxMs) * d.scale
      );
    case 'distance':
      return clamp(d.baseMs + d.msPerPt * distance, d.minMs, d.maxMs);
  }
}

/** Scale a style's own arc by the `arcSize` knob and shift its flow by `arcFlow`. */
export function knobShape(params: MotionParams, arcSize: number, arcFlow: number): ArcShape {
  return {
    startHandle: params.startHandle,
    endHandle: params.endHandle,
    arcSize: arcSize * (params.arcSize / DEFAULT_PARAMS.arcSize),
    arcFlow: Math.min(Math.max(arcFlow + params.arcFlow, -1), 1),
  };
}

/**
 * The spec behind a built-in style. `magnetic`, `adaptive` and `classic` are
 * simulations rather than specs and return `null`.
 */
export function specForStyle(
  style: MotionStyle,
  params: Partial<MotionParams> = {}
): MotionSpec | null {
  const p = motionParams({ ...params, style });
  const base = (path: PathShape, ease: Ease, settle: Settle, duration: Duration): MotionSpec => ({
    path,
    ease,
    settle,
    duration,
    heading: 'tangent',
    effects: resolvedEffects(p),
    trail: { ...DEFAULT_TRAIL },
  });
  switch (style) {
    case 'signature_arc':
      return base(
        { type: 'arc', ...knobShape(p, 0.16, 0.15) },
        'min_jerk',
        { type: 'follow_through', amount: 0.018, maxPt: 8, at: 0.82 },
        fittsDuration(1.1)
      );
    case 'spring_settle':
      return base(
        { type: 'arc', ...knobShape(p, 0.12, 0) },
        'min_jerk',
        {
          type: 'spring',
          amount: 0.05,
          maxPt: 6,
          cycles: 1.3,
          decay: 2.6,
          start: 0.55,
          glideEnd: 0.68,
        },
        fittsDuration(1.35)
      );
    case 'comet_swoop':
      return base(
        { type: 'arc', ...knobShape(p, 0.24, 0.2) },
        'in_out_cubic',
        { type: 'none' },
        fittsDuration(1.15)
      );
    default:
      return null;
  }
}

/** `signature_arc` as a spec: a good starting point for your own. */
export const defaultSpec = (): MotionSpec => specForStyle('signature_arc')!;
