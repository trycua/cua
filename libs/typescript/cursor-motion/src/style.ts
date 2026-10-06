/** The six built-in styles, as Cua Driver names them. */
export type MotionStyle =
  | 'signature_arc'
  | 'spring_settle'
  | 'magnetic'
  | 'comet_swoop'
  | 'adaptive'
  | 'classic';

export const MOTION_STYLES: MotionStyle[] = [
  'signature_arc',
  'spring_settle',
  'magnetic',
  'comet_swoop',
  'adaptive',
  'classic',
];

/** `native` (the style's own), `fitts` or `fixed`. */
export type MotionTiming = 'native' | 'fitts' | 'fixed';
export const MOTION_TIMINGS: MotionTiming[] = ['native', 'fitts', 'fixed'];

export type EffectName = 'trail' | 'glow' | 'magnet' | 'ripple' | 'squish';
export const EFFECT_NAMES: EffectName[] = ['trail', 'glow', 'magnet', 'ripple', 'squish'];

/** Effects that are on or off. */
export type ResolvedEffects = Record<EffectName, boolean>;
/** Per-effect overrides; unset or `null` keeps the style's default. */
export type MotionEffects = Partial<Record<EffectName, boolean | null>>;

export const NO_EFFECTS: ResolvedEffects = {
  trail: false,
  glow: false,
  magnet: false,
  ripple: false,
  squish: false,
};

/** Effects a style turns on when the caller leaves them unset. */
export function defaultEffects(style: MotionStyle): ResolvedEffects {
  const on = (...names: EffectName[]): ResolvedEffects => {
    const out = { ...NO_EFFECTS };
    for (const n of names) out[n] = true;
    return out;
  };
  switch (style) {
    case 'signature_arc':
      return on('glow', 'ripple', 'squish');
    case 'spring_settle':
      return on('glow', 'squish');
    case 'magnetic':
      return on('magnet', 'ripple');
    case 'comet_swoop':
      return on('trail', 'ripple');
    case 'adaptive':
      return on('squish');
    case 'classic':
      return on();
  }
}

export function resolveEffects(overrides: MotionEffects | undefined, base: ResolvedEffects) {
  const out = { ...base };
  for (const n of EFFECT_NAMES) {
    const v = overrides?.[n];
    if (v !== undefined && v !== null) out[n] = v;
  }
  return out;
}

/** Fixed-timing duration when `glideDurationMs` is 0. */
export const DEFAULT_FIXED_MS = 1430;
