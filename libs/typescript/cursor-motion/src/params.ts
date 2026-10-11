import {
  defaultEffects,
  resolveEffects,
  type MotionEffects,
  type MotionStyle,
  type MotionTiming,
  type ResolvedEffects,
} from './style';

/** Knobs of the built-in styles. Defaults match Cua Driver. */
export interface MotionParams {
  style: MotionStyle;
  timing: MotionTiming;
  effects: MotionEffects;
  /** Control-point offset from the start, fraction of the distance. [0, 1] */
  startHandle: number;
  /** Control-point offset from the end. [0, 1] */
  endHandle: number;
  /** Arc scale: 0.25 keeps each style's own arc, 0 is straight. [0, 1] */
  arcSize: number;
  /** Shift of the arc's peak: positive = apex near the destination. [-1, 1] */
  arcFlow: number;
  /** `classic` arrival spring damping: 1 = critical, 0.3 = bouncy. */
  spring: number;
  /** Fixed move time in ms (0 = 1430 for `fixed` timing; > 0 with `native` also fixes it). */
  glideDurationMs: number;
  /** `classic` peak speed, pt/s. */
  peakSpeed: number;
  /** `classic` speed floor at the start, pt/s. */
  minStartSpeed: number;
  /** `classic` speed floor at the end, pt/s. */
  minEndSpeed: number;
  /** `classic` minimum turning radius, pt. */
  turnRadius: number;
}

export const DEFAULT_PARAMS: Readonly<MotionParams> = Object.freeze({
  style: 'signature_arc',
  timing: 'native',
  effects: {},
  startHandle: 0.3,
  endHandle: 0.3,
  arcSize: 0.25,
  arcFlow: 0,
  spring: 0.72,
  glideDurationMs: 0,
  peakSpeed: 900,
  minStartSpeed: 300,
  minEndSpeed: 200,
  turnRadius: 80,
});

export const motionParams = (p: Partial<MotionParams> = {}): MotionParams => ({
  ...DEFAULT_PARAMS,
  ...p,
  effects: { ...(p.effects ?? {}) },
});

export const resolvedEffects = (p: MotionParams): ResolvedEffects =>
  resolveEffects(p.effects, defaultEffects(p.style));
