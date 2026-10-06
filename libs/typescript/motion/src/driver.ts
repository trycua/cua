/**
 * Config snippets: the same motion as TypeScript, and as Cua Driver settings
 * where the driver supports it (the six styles and their knobs).
 */
import { DEFAULT_PARAMS, motionParams, type MotionParams } from './params';
import type { MotionSpec } from './spec';
import { EFFECT_NAMES } from './style';

const round = (v: number) => Math.round(v * 1000) / 1000;

/** Knobs that differ from the defaults, rounded for display. */
function changedKnobs(p: MotionParams): Partial<MotionParams> {
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(DEFAULT_PARAMS) as (keyof MotionParams)[]) {
    if (key === 'effects') continue;
    const v = p[key];
    if (v !== DEFAULT_PARAMS[key]) out[key] = typeof v === 'number' ? round(v) : v;
  }
  const effects = Object.fromEntries(
    EFFECT_NAMES.filter((n) => p.effects[n] != null).map((n) => [n, p.effects[n]])
  );
  if (Object.keys(effects).length) out.effects = effects;
  return out as Partial<MotionParams>;
}

const SNAKE: Record<string, string> = {
  startHandle: 'start_handle',
  endHandle: 'end_handle',
  arcSize: 'arc_size',
  arcFlow: 'arc_flow',
  spring: 'spring',
  glideDurationMs: 'glide_duration_ms',
  turnRadius: 'turn_radius',
};

export interface DriverSnippets {
  /** Saved default for new sessions (style, timing, effects only). */
  configSet: string[];
  /** One session, from the shell. */
  cursorMotion: string;
  /** The `set_agent_cursor_motion` arguments, including the shape knobs. */
  setAgentCursorMotion: Record<string, unknown>;
  /** Knobs the saved default and the shell command cannot carry. */
  callOnly: string[];
}

/** Cua Driver equivalents of built-in style params. */
export function driverSnippets(params: Partial<MotionParams>, session = 'demo'): DriverSnippets {
  const p = motionParams(params);
  const configSet = [
    `cua-driver config set cursor.motion.style ${p.style}`,
    `cua-driver config set cursor.motion.timing ${p.timing}`,
  ];
  const effectArgs: string[] = [];
  for (const n of EFFECT_NAMES) {
    const v = p.effects[n];
    if (v == null) continue;
    configSet.push(`cua-driver config set cursor.motion.effects.${n} ${v}`);
    effectArgs.push(`${n}=${v ? 'on' : 'off'}`);
  }
  let cursorMotion = `cua-driver cursor motion --session ${session} --style ${p.style} --timing ${p.timing}`;
  if (effectArgs.length) cursorMotion += ` --effects ${effectArgs.join(',')}`;
  if (p.glideDurationMs > 0) cursorMotion += ` --glide-ms ${round(p.glideDurationMs)}`;
  const call: Record<string, unknown> = { session, style: p.style, timing: p.timing };
  const callOnly: string[] = [];
  for (const [camel, snake] of Object.entries(SNAKE)) {
    const v = p[camel as keyof MotionParams] as number;
    if (v !== DEFAULT_PARAMS[camel as keyof MotionParams]) {
      call[snake] = round(v);
      if (camel !== 'glideDurationMs') callOnly.push(snake);
    }
  }
  if (effectArgs.length) {
    call.effects = Object.fromEntries(
      EFFECT_NAMES.filter((n) => p.effects[n] != null).map((n) => [n, p.effects[n]])
    );
  }
  return { configSet, cursorMotion, setAgentCursorMotion: call, callOnly };
}

/** TypeScript for planning these params with `@trycua/motion`. */
export function paramsSnippet(params: Partial<MotionParams>): string {
  return [
    "import { planMove } from '@trycua/motion';",
    '',
    `const params = ${JSON.stringify(changedKnobs(motionParams(params)), null, 2)};`,
    'const trajectory = planMove(params, { from, to, target });',
  ].join('\n');
}

/** TypeScript for a custom spec. */
export function specSnippet(spec: MotionSpec): string {
  const r = (v: unknown): unknown =>
    typeof v === 'number'
      ? round(v)
      : v && typeof v === 'object'
        ? Object.fromEntries(Object.entries(v).map(([k, x]) => [k, r(x)]))
        : v;
  return [
    "import { planSpec, type MotionSpec } from '@trycua/motion';",
    '',
    `const spec: MotionSpec = ${JSON.stringify(r(spec), null, 2)};`,
    'const trajectory = planSpec(spec, { from, to, target });',
  ].join('\n');
}
