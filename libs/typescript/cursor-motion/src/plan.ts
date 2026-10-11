import { planDubins } from './dubins';
import { bumpProfile, easeFn, inOutSine, minJerk, wobbleProfile } from './ease';
import { DEFAULT_TRAIL, type TrailSpec } from './effects';
import {
  anchorForPointer,
  dist,
  lerp,
  pointerForAnchor,
  pt,
  wrapAngle,
  type Pt,
  type Rect,
} from './geom';
import { motionParams, resolvedEffects, type MotionParams } from './params';
import { bowPath, cuaPath, naturalSide, straightPath, type Path } from './path';
import { Rng } from './rng';
import { durationMs, specForStyle, type Duration, type Heading, type MotionSpec } from './spec';
import {
  DEFAULT_FIXED_MS,
  NO_EFFECTS,
  type MotionStyle,
  type MotionTiming,
  type ResolvedEffects,
} from './style';

/** Sample period of every planned trajectory, ms (120 Hz). */
export const DT_MS = 1000 / 120;
/** Target box assumed when a move has no element rect. */
export const DEFAULT_TARGET_PT = 24;
/** The arrow's rest heading: pointing up and to the left. */
export const REST_HEADING = Math.PI / 4;
const ARRIVAL_TOLERANCE_PT = 1;
const REDUCED_MOTION_MS = 120;

interface Raw {
  t: number;
  x: number;
  y: number;
}

/** One sample: seconds from the start, the hotspot, and the arrow's heading. */
export interface Sample {
  t: number;
  x: number;
  y: number;
  heading: number;
}

export interface MoveRequest {
  /** Current hotspot. */
  from: Pt;
  /** Requested hotspot. */
  to: Pt;
  /** Current heading; defaults to the rest heading. */
  fromHeading?: number;
  /** Rest heading on arrival; defaults to pi/4. */
  endHeading?: number;
  /** Target element rect, if known. It sets the Fitts width. */
  target?: Rect | null;
  /** Same seed, same motion (only `adaptive` draws from it). */
  seed?: string;
  /** A short straight glide without effects. */
  reducedMotion?: boolean;
}

function sampleTimed(pos: (tau: number) => Pt, ms: number): Raw[] {
  const c = Math.ceil(ms / DT_MS);
  const n = Math.max(Number.isFinite(c) && c > 0 ? c : 0, 2);
  const out: Raw[] = [];
  for (let i = 0; i <= n; i++) {
    const tau = i / n;
    const p = pos(tau);
    out.push({ t: tau * ms, x: p.x, y: p.y });
  }
  return out;
}

function pinEnds(samples: Raw[], from: Pt, aim: Pt): Raw[] {
  if (samples.length) {
    samples[0]!.x = from.x;
    samples[0]!.y = from.y;
    samples[samples.length - 1]!.x = aim.x;
    samples[samples.length - 1]!.y = aim.y;
  }
  return samples;
}

const glide = (from: Pt, aim: Pt, path: Path, profile: (t: number) => number, ms: number) =>
  pinEnds(
    sampleTimed((tau) => path.atFraction(profile(tau)), ms),
    from,
    aim
  );

function speedShaped(from: Pt, aim: Pt, path: Path, shape: (s: number) => number, ms: number) {
  const N = 600;
  const ts = new Float64Array(N + 1);
  for (let i = 1; i <= N; i++) ts[i] = ts[i - 1]! + 1 / Math.max(shape((i - 0.5) / N), 1e-3);
  const total = ts[N]!;
  return pinEnds(
    sampleTimed((tau) => {
      const target = tau * total;
      let lo = 0;
      let hi = N;
      while (hi - lo > 1) {
        const mid = (lo + hi) >> 1;
        if (ts[mid]! < target) lo = mid;
        else hi = mid;
      }
      const f = (target - ts[lo]!) / Math.max(ts[hi]! - ts[lo]!, 1e-9);
      return path.atFraction((lo + f) / N);
    }, ms),
    from,
    aim
  );
}

interface Ctx {
  from: Pt;
  aim: Pt;
  target: Rect;
}

const ctxD = (c: Ctx) => dist(c.from, c.aim);
const targetWidth = (c: Ctx) => Math.max(Math.min(c.target[2], c.target[3]), 4);
const side = (c: Ctx) => naturalSide(c.from, c.aim);

/** Global Fitts timing: `150 + 120 log2(D / W + 1)` ms, 300..1000. */
export function fittsTimingMs(d: number, w: number): number {
  return Math.min(Math.max(150 + 120 * Math.log2(d / Math.max(w, 4) + 1), 300), 1000);
}

function generateSpec(spec: MotionSpec, c: Ctx): Raw[] {
  const s = side(c);
  const sp = spec.path;
  const path =
    sp.type === 'straight'
      ? straightPath(c.from, c.aim)
      : sp.type === 'arc'
        ? cuaPath(c.from, c.aim, { ...sp, arcSize: sp.arcSize * s })
        : bowPath(c.from, c.aim, sp.amount * s);
  const ease = easeFn(spec.ease);
  const ms = durationMs(spec.duration, ctxD(c), targetWidth(c));
  const st = spec.settle;
  switch (st.type) {
    case 'none':
      return glide(c.from, c.aim, path, ease, ms);
    case 'follow_through': {
      const over = Math.min(st.amount, st.maxPt / Math.max(ctxD(c), 1));
      return glide(c.from, c.aim, path, bumpProfile(ease, over, st.at), ms);
    }
    case 'spring': {
      const amp = Math.min(st.amount, st.maxPt / Math.max(ctxD(c), 1));
      const glideEnd = st.glideEnd;
      const profile = wobbleProfile(
        (t) => ease(Math.min(t / glideEnd, 1)),
        amp,
        st.cycles,
        st.decay,
        st.start
      );
      return glide(c.from, c.aim, path, profile, ms);
    }
  }
}

function magnetic(c: Ctx): [Raw[], number] {
  const path = bowPath(c.from, c.aim, 0.04 * side(c));
  const len = path.length;
  const radius = Math.min(40, len * 0.5);
  const pull = 0.45;
  const enterSpeed = 300;
  const out: Raw[] = [{ t: 0, x: c.from.x, y: c.from.y }];
  let s = 0;
  let v = 0;
  let t = 0;
  let snap: number | null = null;
  const dt = DT_MS / 1000;
  while (s < len && t < 4) {
    const rem = len - s;
    if (rem > radius) {
      v = Math.min(1500, v + 7000 * dt, enterSpeed + 5.5 * (rem - radius));
    } else {
      if (snap === null) snap = t * 1000;
      v += 26000 * pull * (radius / Math.max(rem, 6)) * dt;
    }
    s = Math.min(len, s + v * dt);
    t += dt;
    const q = path.atFraction(s / len);
    out.push({ t: t * 1000, x: q.x, y: q.y });
  }
  return [pinEnds(out, c.from, c.aim), snap ?? t * 1000];
}

const plain = (
  path: MotionSpec['path'],
  ease: MotionSpec['ease'],
  duration: Duration
): MotionSpec => ({
  path,
  ease,
  settle: { type: 'none' },
  duration,
  heading: 'tangent',
  effects: { ...NO_EFFECTS },
  trail: { ...DEFAULT_TRAIL },
});

const FITTS_MS: Duration = { type: 'fitts', a: 50, b: 150, minMs: 180, maxMs: 1400, scale: 1 };

const fittsMinjerk = (c: Ctx) =>
  generateSpec(plain({ type: 'bow', amount: 0.02 }, 'min_jerk', FITTS_MS), c);

function keynoteSwoop(c: Ctx, rng: Rng): Raw[] {
  const arc = rng.range(0.25, 0.35);
  return generateSpec(
    plain(
      { type: 'arc', startHandle: 0.3, endHandle: 0.3, arcSize: arc, arcFlow: 0.2 },
      'in_out_cubic',
      { type: 'distance', baseMs: 350, msPerPt: 0.35, minMs: 450, maxMs: 1100 }
    ),
    c
  );
}

function preciseClick(c: Ctx): Raw[] {
  const fp = 0.15;
  const fs = 0.35;
  const shape = (s: number) =>
    s < 0.4
      ? 0.04 + Math.sin((Math.PI * s) / 0.8)
      : s < 1 - fp
        ? 1 - (1 - fs) * inOutSine((s - 0.4) / (0.6 - fp))
        : 0.02 + fs * Math.sqrt(Math.max((1 - s) / fp, 0));
  const path = bowPath(c.from, c.aim, 0.03 * side(c));
  return speedShaped(
    c.from,
    c.aim,
    path,
    shape,
    durationMs(FITTS_MS, ctxD(c), targetWidth(c)) * 1.2
  );
}

/** Which generator `adaptive` picks for a move. */
export function adaptivePick(c: Ctx): 'precise_click' | 'keynote_swoop' | 'fitts_minjerk' {
  if (targetWidth(c) < 16) return 'precise_click';
  if (ctxD(c) > 900) return 'keynote_swoop';
  return 'fitts_minjerk';
}

function retime(samples: Raw[], targetMs: number) {
  const total = samples.length ? samples[samples.length - 1]!.t : 0;
  if (Number.isNaN(total) || total <= 0 || samples.length < 3) return;
  const k = targetMs / total;
  for (const s of samples) s.t *= k;
}

function applyTiming(
  samples: Raw[],
  events: number[],
  c: Ctx,
  timing: MotionTiming,
  fixedMs: number
) {
  const total = samples.length ? samples[samples.length - 1]!.t : 0;
  let want: number;
  if (timing === 'native') return;
  if (timing === 'fixed') want = fixedMs;
  else {
    const last = samples.length ? samples[samples.length - 1]! : c.aim;
    want = fittsTimingMs(dist(c.from, pt(last.x, last.y)), Math.min(c.target[2], c.target[3]));
  }
  if (total > 0) {
    retime(samples, want);
    for (let i = 0; i < events.length; i++) events[i] = (events[i]! * want) / total;
  }
}

/** A planned move, played back by time. */
export class Trajectory {
  constructor(
    readonly samples: Sample[],
    /** When the tip first reaches the target (within 1 pt), seconds. */
    readonly arrivalT: number,
    /** Magnetic lock-on time, seconds. */
    readonly snapT: number | null,
    readonly target: Rect,
    readonly targetKnown: boolean,
    readonly effects: ResolvedEffects,
    readonly trail: TrailSpec,
    /** The built-in style, or `null` for a custom spec. */
    readonly style: MotionStyle | null
  ) {}

  duration(): number {
    return this.samples.length ? this.samples[this.samples.length - 1]!.t : 0;
  }

  end(): Sample {
    return this.samples[this.samples.length - 1]!;
  }

  /** Interpolated sample at `t` seconds, clamped to the ends. */
  sampleAt(t: number): Sample {
    const first = this.samples[0]!;
    if (t <= first.t) return first;
    const last = this.end();
    if (t >= last.t) return last;
    let lo = 0;
    let hi = this.samples.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (this.samples[mid]!.t <= t) lo = mid + 1;
      else hi = mid;
    }
    const a = this.samples[lo - 1]!;
    const b = this.samples[lo]!;
    const f = (t - a.t) / (b.t - a.t);
    return {
      t,
      x: lerp(a.x, b.x, f),
      y: lerp(a.y, b.y, f),
      heading: a.heading + wrapAngle(b.heading - a.heading) * f,
    };
  }

  /** Hotspot velocity at `t`, pt/s. */
  velocityAt(t: number): [number, number] {
    const h = 0.008;
    if (t > this.duration() + h) return [0, 0];
    const a = this.sampleAt(t - h);
    const b = this.sampleAt(t + h);
    return [(b.x - a.x) / (2 * h), (b.y - a.y) / (2 * h)];
  }
}

const TIP_ANGLE = -0.75 * Math.PI;

interface Resolved {
  from: Pt;
  to: Pt;
  fromHeading: number;
  endHeading: number;
  target: Rect;
  targetKnown: boolean;
  seed: string;
}

function resolve(req: MoveRequest): Resolved {
  const r = req.target;
  const known = !!r && r.every((v) => Number.isFinite(v)) && r[2] > 0 && r[3] > 0;
  const h = DEFAULT_TARGET_PT / 2;
  return {
    from: req.from,
    to: req.to,
    fromHeading: req.fromHeading ?? REST_HEADING,
    endHeading: req.endHeading ?? REST_HEADING,
    target: known
      ? (r as Rect)
      : [req.to.x - h, req.to.y - h, DEFAULT_TARGET_PT, DEFAULT_TARGET_PT],
    targetKnown: known,
    seed: req.seed ?? '',
  };
}

function finish(
  rawIn: Raw[],
  snapMs: number | null,
  q: Resolved,
  effects: ResolvedEffects,
  trail: TrailSpec,
  style: MotionStyle | null,
  mode: Heading
): Trajectory {
  const raw = rawIn.length ? rawIn : [{ t: 0, x: q.to.x, y: q.to.y }];
  const n = raw.length;
  let rot = wrapAngle(q.fromHeading - q.endHeading);
  const samples: Sample[] = [];
  for (let i = 0; i < n; i++) {
    const a = raw[Math.max(i - 2, 0)]!;
    const b = raw[Math.min(i + 2, n - 1)]!;
    const dt = Math.max((b.t - a.t) / 1000, 1e-3);
    const vx = (b.x - a.x) / dt;
    const vy = (b.y - a.y) / dt;
    const speed = Math.hypot(vx, vy);
    let want = 0;
    if (mode === 'tangent') {
      const w = Math.min(Math.max((speed - 40) / 260, 0), 1);
      want = wrapAngle(Math.atan2(vy, vx) - TIP_ANGLE) * w;
    }
    const step = i > 0 ? (raw[i]!.t - raw[i - 1]!.t) / 1000 : 0;
    const k = 1 - Math.exp(-step * 22);
    rot += wrapAngle(want - rot) * k;
    samples.push({ t: raw[i]!.t / 1000, x: raw[i]!.x, y: raw[i]!.y, heading: q.endHeading + rot });
  }
  const end = samples[samples.length - 1]!;
  let t = end.t;
  const dt = DT_MS / 1000;
  for (let i = 0; i < 36; i++) {
    if (Math.abs(rot) < 0.002) break;
    t += dt;
    rot -= rot * (1 - Math.exp(-dt * 22));
    samples.push({ ...end, t, heading: q.endHeading + rot });
  }
  samples[samples.length - 1] = { ...samples[samples.length - 1]!, heading: q.endHeading };
  const hit = samples.find((s) => Math.hypot(s.x - q.to.x, s.y - q.to.y) <= ARRIVAL_TOLERANCE_PT);
  const arrival = hit ? hit.t : samples[samples.length - 1]!.t;
  return new Trajectory(
    samples,
    arrival,
    snapMs === null ? null : snapMs / 1000,
    q.target,
    q.targetKnown,
    effects,
    trail,
    style
  );
}

function reduced(q: Resolved, style: MotionStyle | null): Trajectory {
  const raw = glide(q.from, q.to, straightPath(q.from, q.to), minJerk, REDUCED_MOTION_MS);
  return finish(raw, null, q, { ...NO_EFFECTS }, { ...DEFAULT_TRAIL }, style, 'fixed');
}

/** Plan one move for a built-in style. Unset params take Cua Driver's defaults. */
export function planMove(params: Partial<MotionParams>, req: MoveRequest): Trajectory {
  const p = motionParams(params);
  const q = resolve(req);
  const c: Ctx = { from: q.from, aim: q.to, target: q.target };
  const fixedMs = p.glideDurationMs > 0 ? p.glideDurationMs : DEFAULT_FIXED_MS;
  const timing: MotionTiming = p.timing === 'native' && p.glideDurationMs > 0 ? 'fixed' : p.timing;
  if (req.reducedMotion) return reduced(q, p.style);
  if (p.style === 'classic') return planClassic(p, q, timing, fixedMs);
  const spec = specForStyle(p.style, p);
  let raw: Raw[];
  let snap: number | null = null;
  if (spec) raw = generateSpec(spec, c);
  else if (p.style === 'magnetic') [raw, snap] = magnetic(c);
  else {
    const rng = new Rng(q.seed);
    const pick = adaptivePick(c);
    raw =
      pick === 'precise_click'
        ? preciseClick(c)
        : pick === 'keynote_swoop'
          ? keynoteSwoop(c, rng)
          : fittsMinjerk(c);
  }
  const events = snap === null ? [] : [snap];
  applyTiming(raw, events, c, timing, fixedMs);
  return finish(
    raw,
    events.length ? events[0]! : null,
    q,
    resolvedEffects(p),
    { ...DEFAULT_TRAIL },
    p.style,
    p.style === 'magnetic' ? 'fixed' : 'tangent'
  );
}

/** Plan one move for a custom spec. */
export function planSpec(spec: MotionSpec, req: MoveRequest): Trajectory {
  const q = resolve(req);
  if (req.reducedMotion) return reduced(q, null);
  const raw = generateSpec(spec, { from: q.from, aim: q.to, target: q.target });
  return finish(raw, null, q, { ...spec.effects }, { ...spec.trail }, null, spec.heading);
}

function planClassic(
  p: MotionParams,
  q: Resolved,
  timing: MotionTiming,
  fixedMs: number
): Trajectory {
  const K = 400;
  const OVERSHOOT = 0.8;
  const springC = (17 * p.spring) / 0.72;
  const dt = DT_MS / 1000;
  const [ax, ay] = anchorForPointer(q.from.x, q.from.y, q.fromHeading);
  const [tx, ty] = anchorForPointer(q.to.x, q.to.y, q.endHeading);
  const path = planDubins(
    ax,
    ay,
    q.fromHeading + Math.PI,
    tx,
    ty,
    q.endHeading + Math.PI,
    p.turnRadius
  );
  const pathLen = Math.max(path.length, 1);
  const fixed = timing === 'fixed';
  const out: Sample[] = [];
  const push = (t: number, x: number, y: number, heading: number) => {
    const [px, py] = pointerForAnchor(x, y, heading);
    out.push({ t, x: px, y: py, heading });
  };
  push(0, ax, ay, q.fromHeading);
  let d = 0;
  let t = 0;
  let speed = 0;
  let guard = 0;
  while (d < pathLen && guard < 20000) {
    guard++;
    const u = Math.min(d / pathLen, 1);
    const profile = (30 * u * u * (1 - u) * (1 - u)) / 1.875;
    const floor = u < 0.5 ? p.minStartSpeed : p.minEndSpeed;
    speed = fixed ? pathLen / (fixedMs / 1000) : floor + (p.peakSpeed - floor) * profile;
    d += speed * dt;
    t += dt;
    if (d >= pathLen) break;
    const s = path.sample(d);
    push(t, s.x, s.y, s.heading + Math.PI);
  }
  const end = path.sample(pathLen);
  push(t, tx, ty, q.endHeading);
  let arrival = t;
  const impulse = fixed ? p.minEndSpeed : speed;
  let ox = 0;
  let oy = 0;
  let vx = impulse * OVERSHOOT * Math.cos(end.heading);
  let vy = impulse * OVERSHOOT * Math.sin(end.heading);
  for (let i = 0; i < 600; i++) {
    const sdt = dt / 4;
    for (let j = 0; j < 4; j++) {
      vx += (-K * ox - springC * vx) * sdt;
      vy += (-K * oy - springC * vy) * sdt;
      ox += vx * sdt;
      oy += vy * sdt;
    }
    t += dt;
    if (Math.hypot(ox, oy) < 0.3 && Math.hypot(vx, vy) < 2) break;
    push(t, tx + ox, ty + oy, q.endHeading);
  }
  push(t + dt, tx, ty, q.endHeading);
  if (timing === 'fitts') {
    const total = out[out.length - 1]!.t;
    if (total > 0) {
      const want = fittsTimingMs(dist(q.from, q.to), Math.min(q.target[2], q.target[3])) / 1000;
      const k = want / total;
      for (const s of out) s.t *= k;
      arrival *= k;
    }
  }
  return new Trajectory(
    out,
    arrival,
    null,
    q.target,
    q.targetKnown,
    resolvedEffects(p),
    { ...DEFAULT_TRAIL },
    'classic'
  );
}
