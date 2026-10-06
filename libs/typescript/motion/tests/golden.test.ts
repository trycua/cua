// The TypeScript port must reproduce the Rust crate's golden trajectories:
// libs/cua-driver/rust/crates/cua-motion/fixtures/golden.json.
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  easeFn,
  effects,
  hashString,
  planMove,
  planSpec,
  Rng,
  anchorForPointer,
  type MotionParams,
  type MotionSpec,
  type MoveRequest,
  type Trajectory,
} from '../src/index';

const golden = JSON.parse(
  readFileSync(
    new URL('../../../cua-driver/rust/crates/cua-motion/fixtures/golden.json', import.meta.url),
    'utf8'
  )
);

/** Positions and times agree to a micro-point; the fixture rounds to 1e-9. */
const TOL = 1e-6;

const camel = (k: string) => k.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
function camelKeys(v: unknown): any {
  if (Array.isArray(v)) return v.map(camelKeys);
  if (v && typeof v === 'object') {
    return Object.fromEntries(Object.entries(v).map(([k, x]) => [camel(k), camelKeys(x)]));
  }
  return v;
}

function request(r: any): MoveRequest {
  return {
    from: { x: r.from[0], y: r.from[1] },
    to: { x: r.to[0], y: r.to[1] },
    fromHeading: r.from_heading,
    endHeading: r.end_heading,
    target: r.target,
    seed: r.seed,
    reducedMotion: r.reduced_motion,
  };
}

let worst = 0;
function near(path: string, got: number, want: number) {
  const d = Math.abs(got - want);
  worst = Math.max(worst, d);
  if (!(d <= TOL)) throw new Error(`${path}: ${got} vs ${want} (off by ${d})`);
}

function checkFrame(path: string, got: effects.EffectFrame, want: any) {
  const list = (v: any, keys: string[]) => (v ? keys.map((k) => v[k]) : null);
  const pairs: [string, number[] | null, number[] | null][] = [
    ['glow', list(got.glow, ['x', 'y', 'r', 'alpha']), want.glow],
    ['magnet', got.magnet ? [...got.magnet.rect, got.magnet.glow] : null, want.magnet],
    ['ripple', list(got.ripple, ['x', 'y', 'r', 'width', 'alpha']), want.ripple],
  ];
  for (const [name, g, w] of pairs) {
    expect(g === null, `${path}.${name} presence`).toBe(w === null);
    g?.forEach((v, i) => near(`${path}.${name}[${i}]`, v, w![i]!));
  }
  near(`${path}.squish`, got.squish, want.squish);
  expect(got.trail.length, `${path}.trail length`).toBe(want.trail.length);
  got.trail.forEach((s, i) =>
    [s.a[0], s.a[1], s.b[0], s.b[1], s.width, s.alpha].forEach((v, j) =>
      near(`${path}.trail[${i}][${j}]`, v, want.trail[i][j])
    )
  );
}

function check(name: string, traj: Trajectory, out: any) {
  expect(traj.samples.length, `${name}: sample count`).toBe(out.samples);
  near(`${name}.duration`, traj.duration(), out.duration);
  near(`${name}.arrival`, traj.arrivalT, out.arrival_t);
  expect(traj.snapT === null, `${name}: snap`).toBe(out.snap_t === null);
  if (traj.snapT !== null) near(`${name}.snap`, traj.snapT, out.snap_t);
  traj.target.forEach((v, i) => near(`${name}.target[${i}]`, v, out.target[i]));
  expect(traj.targetKnown).toBe(out.target_known);
  expect(traj.effects).toEqual(out.effects);
  const d = traj.duration();
  out.grid.forEach((w: number[], i: number) => {
    const s = traj.sampleAt((d * i) / golden.grid);
    near(`${name}.grid[${i}].t`, s.t, w[0]!);
    near(`${name}.grid[${i}].x`, s.x, w[1]!);
    near(`${name}.grid[${i}].y`, s.y, w[2]!);
    near(`${name}.grid[${i}].heading`, s.heading, w[3]!);
  });
  if (out.frames) {
    near(`${name}.linger`, effects.linger(traj), out.linger);
    // Frames at the same fractions of the move as export_golden.rs.
    out.frames.forEach((f: any, i: number) => {
      const k = [0.2, 0.5, 0.8, 1.0][i]!;
      const t = d * k;
      near(`${name}.frame[${i}].t`, t, f.t);
      const s = traj.sampleAt(t);
      const pos = anchorForPointer(s.x, s.y, s.heading);
      const frame = effects.motionFrame(traj, t, pos, true);
      effects.addClick(
        frame,
        { trail: false, glow: false, magnet: false, ripple: true, squish: true },
        0.03 + 0.4 * k,
        [s.x, s.y]
      );
      checkFrame(`${name}@${f.t}`, frame, f.frame);
    });
  }
}

describe('golden trajectories shared with the Rust crate', () => {
  it('seeded randomness is bit-identical', () => {
    for (const r of golden.rng) {
      expect(hashString(r.seed)).toBe(r.hash);
      const rng = new Rng(r.seed);
      for (const v of r.values) expect(rng.next()).toBe(v);
    }
  });

  it('speed curves match', () => {
    for (const e of golden.eases) {
      const ease = camelKeys(e.ease);
      const f = easeFn(ease);
      e.values.forEach((v: number, i: number) =>
        near(`${JSON.stringify(ease)}[${i}]`, f(i / 10), v)
      );
    }
  });

  it(`built-in styles match (${golden.cases.length} cases)`, () => {
    for (const c of golden.cases) {
      const params = camelKeys(c.params) as MotionParams;
      check(c.name, planMove(params, request(c.request)), c.out);
    }
  });

  it(`custom specs match (${golden.spec_cases.length} cases)`, () => {
    for (const c of golden.spec_cases) {
      const spec = camelKeys(c.spec) as MotionSpec;
      check(c.name, planSpec(spec, request(c.request)), c.out);
    }
    console.log(`worst deviation from Rust: ${worst.toExponential(2)}`);
  });
});
